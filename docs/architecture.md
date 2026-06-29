# chess-base — Architecture

## Overview

chess-base is a single Rust binary that serves a Vue 3 SPA (embedded at build
time) and a JSON API, backed by either embedded SQLite (local mode) or Postgres
(server mode). It collects games from Lichess and Chess.com, imports master PGN
databases, stores and searches them (including by board position), supports
commented PGN studies, and integrates UCI engines.

## Components

### Backend (`src/`)

| Module | Responsibility | I/O |
|---|---|---|
| `position` | FEN/SAN/UCI parsing, legal moves, move application & game replay, Zobrist hashing (shakmaty); variant-aware via threaded `CastlingMode` (Standard / Chess960) | none (pure) |
| `pgn_tree` | Study move-tree: variations, comments, NAGs; `pgn` submodule streams standard PGN ⇄ `MoveTree` (`from_pgn`/`to_pgn`, SAN validated via `position`) | none (pure) |
| `openings` | ECO classification: embedded lichess `chess-openings` dataset → O(1) `zobrist -> (eco, name)` lookup; classifies a game by the longest match along its mainline (`eco_of_position`, `classify_mainline`) | none (pure) |
| `plans` | Engine-PV → per-piece trajectories (`plan_from_pv`, ADR 0017): traces only the start FEN's side-to-move, chaining moves by square continuity into `Trajectory{piece,squares}` paths (`g1→f3→g5`); opponent replies applied but not traced; `max_moves`-capped, panic-free | none (pure) |
| `features` | Factual position **feature tags** (`features_of_fen`, #33): material census + balance, game phase, side to move, check/mate/stalemate, insufficient material, mobility, castling rights, plus a short human-readable `tags` list; grounded facts the interactive analysis tool hands the model (deeper pawn-structure classification #30 layers on) | none (pure) |
| `db` | SeaORM connection, entities, migrations; SQLite/Postgres selection | DB |
| `databases` | Transport-agnostic `DatabaseService`: collection CRUD (create/list/get/rename/delete) over the `databases` table; `kind ∈ {lichess,chesscom,master,own}`, `index_depth` derived from `kind`. Ownership read scope + write guards (ADR 0007/0011) — global (`owner_id IS NULL`) create/mutate requires admin. HTTP routes (`databases/routes.rs`, `/api/databases`) are thin callers | DB |
| `studies` | Transport-agnostic `StudyService`: study lifecycle CRUD (`create`/`list`/`get`/`rename`/`delete`) + PGN import/export (`import_pgn`/`export_pgn` via `pgn_tree::pgn`, issue #9) + node-level `MoveTree` mutations (`add_move`/variation SAN-validated via `position::legal_sans`, `annotate`, `promote_variation`/`reorder_variation`/`delete_node`, issue #18); ownership read scope + write guards (ADR 0007/0011). Pure of HTTP/MCP — both the HTTP routes (`studies/routes.rs`, `/api/studies`) and the scoped MCP study tools (`server/routes/mcp_tools.rs`, issue #17 / ADR-0016) are thin callers | DB |
| `settings` | Transport-agnostic `SettingsService`: per-user UI preferences (theme, board theme, piece set, default database) stored as one JSON blob per user under a `user_settings:{id}` key in the key/value `settings` table — no new entity. Validates the theme value and that `default_database_id` is visible to the caller (own ∪ global). HTTP routes (`settings/routes.rs`, `GET/PUT /api/settings`) are thin callers | DB |
| `auth` | Server-mode auth (ADR 0015): `users`/`sessions` tables, Argon2 hashing, transport-agnostic `AuthService` (register/login/logout/authenticate), `/api/auth/*` routes. Inert in local mode | DB |
| `ingest` | Shared game-ingest path (`ingest_pgn`): parses a PGN, dedups players/event, stores the game, replays the mainline via `position::replay`, and bulk-inserts the `position_index` rows (one per ply, capped by the database's `index_depth`; ADR-0003). One transaction per game; every collector funnels through it | DB |
| `search` | Transport-agnostic search services. `PositionSearchService` (ADR-0003): "find games reaching this position" (`games_with_position`) and the opening tree of aggregated per-continuation stats (`opening_tree`: count + W/D/L), both keyed on the Zobrist `position_index`. `HeaderSearchService` (issue #6, `search/headers.rs`): query games by player/color/event/ECO-prefix/date-range/result, keyset-paginated on a stable `(sort, id)` cursor. Both scope to own ∪ global databases via `databases.owner_id`. The `report` submodule (`PositionReportService`, #28) layers the **pre-chewed** query surface on top of position search — reusing `opening_tree`/`games_with_position` and adding ECO (`openings`), per-move frequency/score and transpositions (distinct move orders reaching a Zobrist) — exposed as internal batch functions and the MCP DB tools. HTTP routes (`search/routes.rs`): `GET /api/search/{tree,games}` stream NDJSON, `GET /api/search/headers` returns a `{ games, next_cursor }` JSON page; thin callers of the services. The SPA's search surface is `SearchView` (issue #69, see "Search UI") | DB |
| `games` | Transport-agnostic `GameService` (issue #68): keyset-paginated `list` of the games in a database (`GameSummary` rows, ordered by id; cursor + clamped `limit`) and single-game `get` (`GameDetail` with PGN movetext + `variant`/`start_fen` for board playback). Visibility follows ownership (own ∪ global). HTTP routes (`games/routes.rs`, `GET /api/games?database_id=…&after=…&limit=…` and `GET /api/games/{id}`) are thin callers | DB |
| `collectors` | `GameSource` trait + Lichess / Chess.com adapters, sync cursor | HTTP |
| `engine` | UCI engine config + message parsing (`command`/`analysis` pure), the `manager::Engine` process manager (spawn, handshake, `setoption`, `position`/`go`/`stop`, streamed analysis), the pooled `service::EngineService` facade — one-shot `analyse` for batch + MCP (ADR 0014) — and the `download` auto-download manager (platform catalog → fetch + checksum + register, #11) (Stockfish, Lc0/Maia) | process / HTTP |
| `ai/llm` | Provider-agnostic LLM client: `LlmProvider` trait + Anthropic Messages API client (ADR 0013); HTTP behind an injectable `Transport` seam | HTTP |
| `server` | Axum router, app state, request identity, MCP `/mcp` endpoint + its auth (OAuth 2.1 / service token, ADR 0016), engine analysis WebSocket, embedded SPA, browser launch, lifecycle | HTTP |

The **pure** modules (`position`, `pgn_tree`, `openings`, `plans`) carry the chess logic and are unit-tested without any
runtime. Everything else is a thin adapter with dependencies injected, so the
business logic stays testable and reusable across transports (HTTP and the MCP
`/mcp` endpoint).

### Frontend (`frontend/`)

Vue 3 + Vite + Pinia + Tailwind v4. Board rendering via **chessground**;
client-side move legality via **chess.js**. Built to `frontend/dist` and embedded
into the binary with `rust-embed` (`src/server/embed.rs`). `build.rs` guarantees
the folder exists so the crate always compiles even before the SPA is built.

`App.vue` is a thin nav/layout shell around a `<router-view>`; **vue-router**
(`router/index.js`, HTML5 history) maps each top-level surface to a lazily-loaded
view in `views/`: `AnalysisView` (`/`, the board + analysis panel), `GamesView`
(`/games`, the game browser), `SearchView` (`/search`, see "Search UI" below)
and `LoginView` (`/login`, the server-mode register/login form, see "Auth UI"
below) plus a stub for `collections`. Deep links work because the server's
`static_handler` falls back to `index.html` for unknown paths
(`src/server/routes/mod.rs`).

State lives in Pinia stores: `stores/game.js` (chess.js-backed position,
legal-move `dests`, play-vs-engine moves), `stores/games.js` (the game browser —
keyset-paginated list for a selected database plus the opened game's replay state;
backed by `/api/games`), `stores/engine.js` (the
`/api/engine/analyse` WebSocket — folds streamed `info`/`bestmove` events into
reactive eval/PV state; the socket factory is injectable for tests) and
`stores/settings.js` (per-user UI preferences with a `localStorage` mirror for
instant load; see "User settings" below) and `stores/auth.js` (server-mode
session: register/login/logout + the resolved caller; see "Auth UI" below). The
WebSocket protocol parsing/formatting is isolated in the pure, unit-tested
`lib/engineStream.js` (and `lib/pv.js` for UCI→SAN). Replaying a stored game's
PGN into one board position per ply, plus the pure ply-navigation logic, lives in
the unit-tested `lib/pgnViewer.js`. `components/AnalysisPanel.vue`
(+ `EvalBar.vue`) renders the eval bar, MultiPV lines, depth/nps, engine options
and play-vs-engine controls; `Board.vue` is presentational (it also drives the
read-only game viewer in `GamesView`) and emits user moves.

In dev, Vite serves the SPA and proxies `/api` (with `ws: true` for the engine
WebSocket) to the backend on `:3030`.

## Run modes

`Mode::Local` → SQLite + auto-open browser + single implicit admin user.
`Mode::Server` → Postgres + multi-user. Selected in `src/bin/chess-base.rs` from
CLI flags; resolved into `AppConfig` (config) → `AppState` (runtime).

## Request identity (ADR 0011)

`server/identity.rs` defines `CurrentUser { id, is_admin }` — the one identity
type every service takes — produced by an Axum extractor. Resolution is the only
mode-dependent part and lives in `AppState::resolve_current_user`: local mode is
always the implicit admin (`local-admin`); server mode resolves the session token
through `auth::AuthService` (#14, ADR 0015). Two shared helpers enforce the
ownership model (ADR 0007) in one place: `scope(owner_col, user)` (the
`owner == caller OR owner IS NULL` read filter) and `assert_admin(user)`. The
`/api/whoami` route exposes the resolved caller to the SPA.

## Server-mode auth (ADR 0015)

`auth/` is the server-mode-only authentication layer (inert in local mode). It
owns `users` (accounts + `is_admin` role) and `sessions` (opaque tokens) over
migration `m0003_auth`, Argon2 password hashing (`auth/password.rs`), and the
transport-agnostic `AuthService` (register / login / logout / `authenticate`).
A request carries its token as `Authorization: Bearer <token>` or a `session`
cookie; both resolve via `auth::token_from_headers`. The HTTP surface
(`auth/routes.rs`) is `POST /api/auth/{register,login,logout}`. Bootstrap rule:
the **first** registered user is made admin, so global databases are manageable.
`AuthService` is the only thing #14 added to the identity seam — no handler or
service signature changed.

### Auth UI (#71)

The SPA's auth surface is server-mode only; local mode is the implicit admin and
shows no login controls. `api.js` keeps the session token in memory plus a
`localStorage` mirror (`setAuthToken`/`getAuthToken`) and attaches it as
`Authorization: Bearer <token>` to every request (the HttpOnly `session` cookie
the backend sets still works too — the Bearer header just lets the client decide
when it authenticates and drop it on logout). `stores/auth.js` resolves the run
mode from `/api/health` once (`init()`), restores the user via `/api/whoami` when
a token is present, and exposes `register`/`login`/`logout` plus `needsAuth`. The
router guard (`authRedirect` in `router/index.js`) bounces gated navigations to
`LoginView` (`/login`) with a `redirect` query and sends already-authenticated
callers away from it. Backend error messages are pre-sanitized (generic for 5xx),
so the form surfaces them verbatim without leaking internals.

## MCP endpoint (ADR 0008)

`server/routes/mcp.rs` is a hand-rolled JSON-RPC 2.0 endpoint at `POST /mcp`
(protocol `2025-03-26`), no MCP server crate. It is transport/dispatch plumbing:
`initialize` (serverInfo + capabilities + instructions), `tools/list`,
`tools/call`, and the `notifications/initialized` ack. A `ToolRegistry` holds
`Tool`s (name + JSON input schema + async handler); each handler returns a
`ToolOutcome` the dispatcher wraps into the MCP `content`/`isError` envelope.
Unknown method → `-32601`, unknown tool → `-32602`. The tool builders live in
`server/routes/mcp_tools.rs`: an `echo` stub proves dispatch, the engine facade
registers `engine_analyse` (#27, see ADR 0014), and the study tools
(`study_create` / `study_add_move` / `study_annotate`, #17) edit the caller's
studies through `StudyService`. The **pre-chewed DB tools** live in
`server/routes/mcp_db_tools.rs` (#28): `db_position_report` (ECO + per-move
win/draw/loss with frequency/score + transpositions) and `db_reference_games`
(scoped reference games), both thin callers of `search::PositionReportService`
returning synthesized JSON the LLM consumes but never recomputes (ADR-0009). The
**interactive analysis tool** lives in `server/routes/mcp_analysis.rs` (#33):
`analyse_position` is the one-shot "explain this position" entry point — it
bundles the engine eval/PV, the `db_position_report`, and the pure
`features::features_of_fen` feature tags (material, game phase, check/mate,
castling rights) into a single grounded snapshot so a connected client cites tool
output rather than inventing lines. A missing engine leaves `engine: null` with an
explanatory note; the DB report and features are always present. The unbundled
tools stay available for an agent that wants to drill in further.

Every `/mcp` call is **authenticated** (ADR 0016): `server/auth.rs::authenticate_mcp`
resolves an OAuth access token then a service token to the one `CurrentUser`, which
is threaded into each handler so a tool scopes its reads/writes to the caller (the
study write-guard rejects mutating a non-owned study). A miss returns `401` with
`WWW-Authenticate: Bearer resource_metadata="…"`.

## MCP auth: OAuth 2.1 + service token (ADR 0016)

`server/routes/oauth.rs` is the OAuth 2.1 authorization server and discovery
metadata for `/mcp`. claude.ai self-onboards: dynamic client registration
(`POST /oauth/register`, RFC 7591, public/PKCE-only), the authorization-code grant
(`GET /oauth/authorize` → `POST /oauth/token`, PKCE **S256**) and the
`refresh_token` grant, with `/.well-known/oauth-protected-resource` (RFC 9728) and
`/.well-known/oauth-authorization-server` (RFC 8414) built from the request host.
`authorize` requires a logged-in server-mode session and **auto-consents** (an
anonymous request bounces to the SPA login). Local mode skips OAuth: it seeds and
prints a static **service token** at startup (the `claude mcp add … --header
"Authorization: Bearer …"` line), reused across restarts. Both grants resolve to a
`CurrentUser`; authorization is by resource ownership (ADR 0007), not OAuth scopes.

## Engine analysis (ADR 0012)

The `engine` module is split so the chess-specific parts stay pure and unit-tested
while process I/O is a thin adapter:

- `engine/command.rs` — builds the UCI command text (`position`, `go`, `setoption`)
  from a `Limits` model; no I/O.
- `engine/analysis.rs` — maps parsed `vampirc-uci` messages into the flat,
  serializable `AnalysisEvent` (`info` / `bestmove`) the SPA consumes; no I/O.
- `engine/manager.rs` — `Engine` owns a Tokio child process, runs the
  `uci`/`isready` handshake on spawn (`kill_on_drop`, so a dropped handle never
  leaks a process), applies `setoption`, drives `position`/`go`/`stop`, and yields
  `AnalysisEvent`s one at a time via `next_event`. Callers own the read loop.

`server/engine_ws.rs` exposes `GET /api/engine/analyse`, an authenticated
(`CurrentUser`) WebSocket. It resolves the engine through the registry default
(or a `?engine=<name>` override; absent/none ⇒ `503`) and spawns it for the
socket's lifetime, `select!`ing between client control messages
(`{"type":"analyse",…}` / `{"type":"stop"}`) and streamed engine events, restarting
cleanly (stop → drain → re-`go`) when a new position arrives mid-search. It defaults
`MultiPV` to `3` when the client omits it, and additively enriches each PV-bearing
`info` with a `{"type":"planline",…}` frame (`multipv`/`depth`/`score`/`pv` plus
`plan_from_pv` `trajectories`, ADR 0017) for the Plans overlay — the bare `info`
event is still sent unchanged, and a plan-computation failure degrades to empty
`trajectories` rather than dropping the line. A real engine is integration-tested
behind `CHESS_BASE_TEST_ENGINE` (skipped if unset); the `planline` wiring and its
fallback are unit-tested without a process.

### Engine registry — persisted multi-engine config (ADR 0005 amendment)

`engine/registry.rs` adds `EngineRegistry`, a transport-agnostic service that
persists several `EngineConfig`s plus a `default_engine` selector in the key/value
`settings` store (no new entity). `EngineConfig` carries an optional `runner` —
a launch wrapper (script, `wine`, `docker exec` shim) prepended to the binary, so
the engine spawns as `<runner> <path> …`. `resolve_default` applies the resolution
order (first wins): a user-configured registry default → the embedded
`bundled-stockfish` build (still a seam returning `None`) → an auto-downloaded
binary (#11, persisted under the `downloaded_engines` settings key, kept apart
from the user-facing `engines` list). `server/routes/engines.rs`
exposes the CRUD (`GET/POST /api/engines`, `DELETE /api/engines/{name}`,
`GET/PUT /api/engines/default`); reads are open to any caller, writes are
admin-gated. `AppState` resolves through `state.engines()` rather than holding an
engine field; `--engine` / `CHESS_BASE_ENGINE` seeds the registry at startup
without clobbering a persisted selection. The WebSocket and MCP tools are thin
callers. The frontend `EnginesSettings.vue` panel (Settings toggle) drives the CRUD.

### Engine auto-download manager (ADR 0005 / #11)

`engine/download.rs` fetches the default engines so the app works out of the box
without manual paths. `Platform::detect()` reads `std::env::consts` (`os`/`arch`);
`catalog(platform)` maps it to a `Plan` — the Stockfish binary plus, where Lc0 is
available, the Lc0 binary and Maia-1100 `.pb.gz` weights (which Lc0 reads natively,
so no extraction). `Manager<F: Fetch>` installs a plan into the engines dir:
download → SHA-256 `verify_checksum` (mismatch rejected, nothing installed) →
temp-file + atomic rename, marking binaries executable on Unix. It is idempotent —
an asset already on disk with a matching checksum is not re-fetched — and every
failure is an `Err`, never a panic. The network is the `Fetch` trait seam
(`HttpFetcher` over `reqwest` in prod; a synthetic fetcher in tests, mirroring the
LLM `Transport` seam), so no real downloads run in the suite. At startup `serve`
calls `download_default_engines(engines_dir)` **best-effort** when
`--no-engine-download` is unset and no engine already resolves, persisting the
result via `EngineRegistry::set_downloaded`; a failure is logged and the server
still starts. The engines dir is `--engines-dir` / `CHESS_BASE_ENGINES_DIR`
(default `engines/`); individual engine paths remain overridable through the
registry settings.

### User settings — per-user UI preferences (issue #13)

`settings/mod.rs` adds `SettingsService`, a transport-agnostic service that
persists each user's UI preferences (theme, board theme, piece set, default
database) as a single JSON blob under a `user_settings:{id}` key in the key/value
`settings` table (no new entity), so local mode (single implicit admin) and
server mode (many users) share one storage path. It validates the theme against
a known set and that `default_database_id` is visible to the caller (own ∪
global, via the shared `scope` helper); blank string fields normalize to absent.
`server/routes/settings.rs` exposes `GET/PUT /api/settings` (the caller's own
settings; gated by the standard identity extractor, so server mode requires
auth). The frontend `stores/settings.js` Pinia store mirrors the server into
`localStorage` so the last-known preferences render instantly on load, then
reconciles with the backend (the source of truth); `components/SettingsView.vue`
(Settings toggle, which also embeds `EnginesSettings.vue`) drives it, and the
resolved theme/board theme are applied to the document and `Board.vue`.

### Engine facade — one pool, two consumption paths (ADR 0014)

`engine/service.rs` adds `EngineService`, a small bounded pool over `Engine` with
one one-shot method, `analyse(fen, limits, options) -> Analysis` (flat
eval/PV/bestmove). The same pooled service backs **two facades**:

- the **batch pipeline** calls `analyse` directly in-process — the returned
  eval/PV is plain Rust data and never enters any LLM context (the ADR-0009
  guard);
- the **MCP endpoint** registers an `engine_analyse` tool that routes through the
  *same* `analyse`, for interactive analysis by a connected client.

`AppState.engine_service` holds an `Arc<EngineService>` built at startup from the
registry's resolved default (`None` ⇒ both facades disabled; the MCP tool answers an
`isError` outcome, batch callers get nothing to call). The pool spawns engines
lazily, reuses idle ones, and caps live processes with a semaphore. The
streaming WebSocket keeps its own per-socket engine: it needs incremental `info`
updates and a mid-search `stop`, which the one-shot pool deliberately does not
model. The event-folding is pure and unit-tested; the live pool and MCP tool are
integration-tested behind `CHESS_BASE_TEST_ENGINE`.

### Plan trajectories — engine PV → per-piece paths (ADR 0017)

`plans.rs` (pure) turns a streamed principal variation into a **Plan**: the
*idea* behind a line, drawn as per-piece arrows. `plan_from_pv(start_fen, pv_uci,
max_moves, mode)` traces **only** the start FEN's side-to-move — opponent replies
are applied to keep the board legal but never traced — and chains its moves by
square continuity: a move whose origin is an existing trajectory's current square
extends it (`g1→f3` then `f3→g5`), else it starts a new path. Captures keep the
chain; castling traces the king's path (`e1→g1`). `max_moves` caps the side's own
plies (default 4) for readable arrows, and the function is panic-free — only an
invalid `start_fen` errors; a truncated or illegal PV returns what it could
trace. Built on `position` (FEN/UCI parsing), it is unit-tested with no engine.
The engine WebSocket emission and the future MCP endpoint are thin callers; the
frontend only renders the `serde`-serialized `Plan`.

## LLM provider (ADR 0013)

`ai/llm` is the provider-agnostic Claude client shared by the batch annotation
pass (Epic 9) and the interactive assistant (Epic 7). `LlmProvider::complete`
takes provider-agnostic `Message`s (user / assistant-with-tool-calls /
tool-results) plus optional `ToolSpec`s and returns text and/or `ToolCall`s — the
same surface the interactive assistant reuses for tool-calling. The only concrete
provider today is `anthropic::AnthropicProvider` over `POST /v1/messages`; a small
trait keeps room for others. The HTTP boundary is the `Transport` trait, so wire
encoding and response parsing are unit-tested against a stub with no network (the
one live test is gated behind `ANTHROPIC_API_KEY`). The model id is configurable —
default Sonnet-class for cost, Opus by override. **The API key is server-side
only**: it travels in the `x-api-key` header and never reaches the SPA.

## Data model

- `settings(key, value)` — app/user key-value settings.
- `databases(id, owner_id?, name, kind, index_depth?)` — an ownable collection of
  games. `owner_id IS NULL` ⇒ a **global**, admin-managed database visible to all
  users; `kind ∈ {lichess, chesscom, master, own}`. Search scope for a user is
  *their* databases ∪ all global databases. `index_depth` is the per-DB position-index
  policy (ADR 0003): `NULL` ⇒ index every ply (the default for own DBs);
  `Some(n)` ⇒ cap `position_index` to the first `n` plies (`entities::databases::
  default_index_depth` returns `Some(36)` for `master`).
- `players(id, name unique)` / `events(id, name unique)` — deduplicated header names.
- `games(id, database_id, white_player_id?, black_player_id?, event_id?, site?,
  round?, date?, result?, eco?, white_elo?, black_elo?, variant, start_fen?,
  ply_count?, pgn?)` — one game with its PGN header roster. `variant` (default
  `standard`) + nullable `start_fen` make Chess960 / set-up positions first-class
  (ADR 0010); `date` is verbatim PGN text (may be partial, `1992.??.??`).
- `position_index(id, zobrist, game_id, ply, move, database_id)` — one row per
  indexed mainline ply (ADR 0003); indexed on `zobrist` for "find games reaching
  this position". `database_id` is denormalized so search filters by scope without a
  join. The Zobrist `u64` is stored as `i64` by a **bit-preserving reinterpret**
  (`u64 as i64`, reversible — see `entities::position_index::{to_i64, from_i64}`),
  since neither backend has an unsigned 64-bit integer.
- `studies(id, database_id, owner_id?, name, tree_json, created_at)` — a named,
  serialized `pgn_tree::MoveTree` (JSON in `tree_json`); `owner_id IS NULL` mirrors
  the global-collection rule.
- `users(id, username, password_hash, is_admin, created_at)` — server-mode accounts
  (ADR 0015); `id` is the string that lands in `owner_id`. `username` is unique;
  `password_hash` is an Argon2 PHC string.
- `sessions(token, user_id, created_at, expires_at)` — opaque bearer/cookie tokens
  with a hard expiry; `user_id` FKs `users.id` (cascade delete). Indexed on `user_id`.
- `service_tokens(token, owner_id, is_admin, label, created_at, expires_at?)` —
  static MCP bearers (ADR 0016): the local-mode printed token and admin-issued
  server tokens. `owner_id` lands in the ownership `scope`; `is_admin` carries the
  role, so a token resolves to a `CurrentUser` without a `users` lookup.
- `oauth_clients(client_id, client_name, redirect_uris, created_at)` — public,
  PKCE-only OAuth clients from dynamic registration (RFC 7591). `redirect_uris` is
  a JSON array.
- `oauth_codes(code, client_id, user_id, redirect_uri, code_challenge,
  code_challenge_method, scope, expires_at, used)` — short-lived, single-use
  authorization codes.
- `oauth_tokens(access_token, refresh_token, client_id, user_id, scope, created_at,
  expires_at)` — issued OAuth pairs; the access token is what `authenticate_mcp`
  checks, the refresh token mints a fresh pair (both rotate on refresh).

Indices cover `zobrist`, the games header columns (`database_id`, player/event FKs,
`date`, `eco`, `result`) and `database_id`/`owner_id` scoping. Migration `m0002_core_schema`
adds the core domain; `m0003_auth` adds `users`/`sessions`; `m0004_oauth` adds the
MCP-auth tables (`service_tokens`, `oauth_clients`, `oauth_codes`, `oauth_tokens`).
All run on both SQLite and Postgres.

### Position search

Every indexed position is keyed by the 64-bit Polyglot-compatible Zobrist hash
produced by `position::zobrist_of_fen`. The same scheme works identically on
SQLite and Postgres (a plain indexed integer column), avoiding a separate
key-value store while covering the self-hosted scale we target.

`search::PositionSearchService` (issue #7) is the query side of that index. A FEN
is hashed to its Zobrist key, then the `position_index` rows at that key — scoped
to the caller's databases plus global ones via the denormalized `database_id` —
are aggregated two ways: `opening_tree` groups the move played from each row into
per-continuation stats (occurrence `count` plus W/D/L from the owning game's
`result`), sorted by count; `games_with_position` takes the distinct games and
resolves their player names. `server/routes`'s `GET /api/search/tree?fen=…` and
`GET /api/search/games?fen=…&limit=…` stream the result rows as NDJSON
(`application/x-ndjson`, one JSON object per line).

`search::HeaderSearchService` (issue #6) is the metadata query side: it filters
`games` by player (resolved through the `players`/`events` name tables, optionally
pinned to white/black), ECO prefix, `Date` range, `Result` and event, scoped the
same way (own ∪ global `databases`). Results are **keyset-paginated** rather than
`OFFSET`-paginated: each page is ordered by a chosen sort column (`date`, NULLs
coalesced to `''`, or `id`) plus the unique `id` tiebreaker, and the last row's
`(sort, id)` key is handed back as an opaque base64url `next_cursor`. The next
request seeks strictly past that key, so page depth costs nothing — page N+1 is a
single indexed range scan. `GET /api/search/headers?player=…&color=…&event=…&eco=…
&date_from=…&date_to=…&result=…&sort=…&dir=…&limit=…&cursor=…` returns one
`{ games, next_cursor }` JSON page (`next_cursor` is `null` once exhausted).

`search::report::PositionReportService` (issue #28) is the **pre-chewed** layer
on top of that query side: it reuses `opening_tree`/`games_with_position`
verbatim and synthesizes a single `PositionReport` per position — the ECO
code+name (`openings::opening_of_zobrist`), each continuation's win/draw/loss
plus derived `frequency` (share of games) and `score` (White's performance), and
the **transpositions** (the distinct move orders that reach the same Zobrist,
reconstructed by replaying each game's indexed moves up to its first arrival).
`references` returns the scoped games for a line/structure. The layer is exposed
only as internal batch functions (`position_report`, `position_reports`,
`references`) and the MCP DB tools — no HTTP route — so the LLM consumes
conclusions it never computes (ADR-0009).

### Search UI

`SearchView` (`/search`, issue #69) toggles between two surfaces. **Header search**
(`components/HeaderSearch.vue`) is a player/color/event/result/ECO/date form whose
results render as a games table with a "Load more" button that follows the
`next_cursor` keyset pages. **Position explorer** (`components/PositionExplorer.vue`)
reuses `Board.vue`: dragging a piece (or clicking a move-stats row) descends the
opening tree, "back"/"start" walk the line, and the table shows each continuation's
frequency and a W/D/L bar alongside the games reaching the position. Both are
driven by `stores/search.js`. The pure, unit-tested logic is split out:
`lib/headerQuery.js` owns the query state (empty shape, blank detection,
snake_case param mapping) and `lib/openingTree.js` owns tree navigation (replay a
SAN line to a FEN + legal `dests` via chess.js, board-drag→SAN, stat math). The
store calls `api.search.{headers,tree,games}` — `headers` returns a JSON page,
`tree`/`games` parse the NDJSON streams.

## Build & CI

`make build` builds the SPA then `cargo build --release`. CI
(`.github/workflows/ci.yml`) builds the frontend, runs frontend tests, then
rustfmt + clippy (`-D warnings`) + cargo build + tests.

## Deployment

- **Local** ships as a self-contained release binary (ADR 0004): the release
  workflow embeds the SPA and publishes one binary per desktop platform.
- **Server** ships as a container (ADR 0016): a multi-stage `Dockerfile` builds
  the SPA, compiles the release binary with it embedded, and runs it from a slim
  Debian image. `docker-compose.yml` runs that image against `postgres:16` with a
  named `pgdata` volume; the app reads `DATABASE_URL`, binds `0.0.0.0:3030`, and
  runs migrations on startup. Credentials/port come from `.env` (`.env.example`).

## Roadmap

See the epics in `.claude/CLAUDE.md`. Each epic is a GitHub milestone; concrete
features are individual issues. Epic 7 adds an MCP JSON-RPC endpoint (mirroring
the `site` project's `routes/mcp.rs`) exposing engine/database/interactive-analysis
tools so an external AI client — or, later, an embedded Claude assistant — can read
and analyse studies. Study *authoring* — lifecycle CRUD + PGN import/export
(issue #9) and node-level create/annotate/restructure (issue #18) — is a separate
programmatic REST API (`/api/studies`), **not** an MCP tool surface, per ADR-0009:
the LLM annotates batches that are committed through that same `StudyService`,
never via runtime MCP mutation.

**Epic 9 — LLM study generation pipeline** is the AI-studies design proper
(see [ADR-0009](decisions/0009-llm-study-pipeline.md)): the LLM is an *annotator*,
with the engine and database as the sole source of chess truth. Code-orchestrated
preprocessing *stages* (variation-tree builder; a pawn-structure/key-square
**feature extractor** — the project's center of gravity) produce a finished, tagged
tree; the LLM annotates it; every concrete claim is verified against engine/DB
before commit. Engine eval/PV never enters the model context in batch mode. The
engine/DB service is exposed both as MCP tools (interactive) and as direct
in-process calls (batch).
