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
| `db` | SeaORM connection, entities, migrations; SQLite/Postgres selection | DB |
| `databases` | Transport-agnostic `DatabaseService`: collection CRUD (create/list/get/rename/delete) over the `databases` table; `kind ∈ {lichess,chesscom,master,own}`, `index_depth` derived from `kind`. Ownership read scope + write guards (ADR 0007/0011) — global (`owner_id IS NULL`) create/mutate requires admin. HTTP routes (`databases/routes.rs`, `/api/databases`) are thin callers | DB |
| `studies` | Transport-agnostic `StudyService`: study CRUD + node-level `MoveTree` mutations (`add_move`/variation SAN-validated via `position::legal_sans`, `annotate`, `promote_variation`/`reorder_variation`/`delete_node`); ownership read scope + write guards (ADR 0007/0011). Pure of HTTP/MCP — both the HTTP routes (`studies/routes.rs`, `/api/studies`, issue #18) and the scoped MCP study tools (`server/routes/mcp_tools.rs`, issue #17 / ADR-0016) are thin callers | DB |
| `auth` | Server-mode auth (ADR 0015): `users`/`sessions` tables, Argon2 hashing, transport-agnostic `AuthService` (register/login/logout/authenticate), `/api/auth/*` routes. Inert in local mode | DB |
| `ingest` | Shared game-ingest path (`ingest_pgn`): parses a PGN, dedups players/event, stores the game, replays the mainline via `position::replay`, and bulk-inserts the `position_index` rows (one per ply, capped by the database's `index_depth`; ADR-0003). One transaction per game; every collector funnels through it | DB |
| `collectors` | `GameSource` trait + Lichess / Chess.com adapters, sync cursor | HTTP |
| `engine` | UCI engine config + message parsing (`command`/`analysis` pure), the `manager::Engine` process manager (spawn, handshake, `setoption`, `position`/`go`/`stop`, streamed analysis) and the pooled `service::EngineService` facade — one-shot `analyse` for batch + MCP (ADR 0014) (Stockfish, Lc0/Maia) | process |
| `ai/llm` | Provider-agnostic LLM client: `LlmProvider` trait + Anthropic Messages API client (ADR 0013); HTTP behind an injectable `Transport` seam | HTTP |
| `server` | Axum router, app state, request identity, MCP `/mcp` endpoint + its auth (OAuth 2.1 / service token, ADR 0016), engine analysis WebSocket, embedded SPA, browser launch, lifecycle | HTTP |

The **pure** modules (`position`, `pgn_tree`, `openings`) carry the chess logic and are unit-tested without any
runtime. Everything else is a thin adapter with dependencies injected, so the
business logic stays testable and reusable across transports (HTTP and the MCP
`/mcp` endpoint).

### Frontend (`frontend/`)

Vue 3 + Vite + Pinia + Tailwind v4. Board rendering via **chessground**;
client-side move legality via **chess.js**. Built to `frontend/dist` and embedded
into the binary with `rust-embed` (`src/server/embed.rs`). `build.rs` guarantees
the folder exists so the crate always compiles even before the SPA is built.

State lives in two Pinia stores: `stores/game.js` (chess.js-backed position,
legal-move `dests`, play-vs-engine moves) and `stores/engine.js` (the
`/api/engine/analyse` WebSocket — folds streamed `info`/`bestmove` events into
reactive eval/PV state; the socket factory is injectable for tests). The
WebSocket protocol parsing/formatting is isolated in the pure, unit-tested
`lib/engineStream.js` (and `lib/pv.js` for UCI→SAN). `components/AnalysisPanel.vue`
(+ `EvalBar.vue`) renders the eval bar, MultiPV lines, depth/nps, engine options
and play-vs-engine controls; `Board.vue` is presentational and emits user moves.

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
studies through `StudyService`.

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
cleanly (stop → drain → re-`go`) when a new position arrives mid-search. A real
engine is integration-tested behind `CHESS_BASE_TEST_ENGINE` (skipped if unset).

### Engine registry — persisted multi-engine config (ADR 0005 amendment)

`engine/registry.rs` adds `EngineRegistry`, a transport-agnostic service that
persists several `EngineConfig`s plus a `default_engine` selector in the key/value
`settings` store (no new entity). `EngineConfig` carries an optional `runner` —
a launch wrapper (script, `wine`, `docker exec` shim) prepended to the binary, so
the engine spawns as `<runner> <path> …`. `resolve_default` applies the resolution
order (first wins): a user-configured registry default → the embedded
`bundled-stockfish` build → an auto-downloaded binary (#11); the latter two are
seams returning `None` until those features land. `server/routes/engines.rs`
exposes the CRUD (`GET/POST /api/engines`, `DELETE /api/engines/{name}`,
`GET/PUT /api/engines/default`); reads are open to any caller, writes are
admin-gated. `AppState` resolves through `state.engines()` rather than holding an
engine field; `--engine` / `CHESS_BASE_ENGINE` seeds the registry at startup
without clobbering a persisted selection. The WebSocket and MCP tools are thin
callers. The frontend `EnginesSettings.vue` panel (Settings toggle) drives the CRUD.

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
and analyse studies. Study *authoring* (node-level create/annotate/restructure) is
a separate programmatic REST API (`/api/studies`, issue #18), **not** an MCP tool
surface, per ADR-0009: the LLM annotates batches that are committed through that
same `StudyService`, never via runtime MCP mutation.

**Epic 9 — LLM study generation pipeline** is the AI-studies design proper
(see [ADR-0009](decisions/0009-llm-study-pipeline.md)): the LLM is an *annotator*,
with the engine and database as the sole source of chess truth. Code-orchestrated
preprocessing *stages* (variation-tree builder; a pawn-structure/key-square
**feature extractor** — the project's center of gravity) produce a finished, tagged
tree; the LLM annotates it; every concrete claim is verified against engine/DB
before commit. Engine eval/PV never enters the model context in batch mode. The
engine/DB service is exposed both as MCP tools (interactive) and as direct
in-process calls (batch).
