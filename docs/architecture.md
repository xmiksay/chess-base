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
| `studies` | Transport-agnostic `StudyService`: study CRUD + `MoveTree` edits (`add_move` SAN-validated via `position::legal_sans`, `annotate`); ownership read scope + write guards (ADR 0007/0011). Pure of HTTP/MCP — the HTTP routes and MCP tools are thin callers | DB |
| `collectors` | `GameSource` trait + Lichess / Chess.com adapters, sync cursor | HTTP |
| `engine` | UCI engine config + message parsing (`command`/`analysis` pure) and the `manager::Engine` process manager: spawn, handshake, `setoption`, `position`/`go`/`stop`, streamed analysis (Stockfish, Lc0/Maia) | process |
| `ai/llm` | Provider-agnostic LLM client: `LlmProvider` trait + Anthropic Messages API client (ADR 0013); HTTP behind an injectable `Transport` seam | HTTP |
| `server` | Axum router, app state, request identity, MCP `/mcp` endpoint, engine analysis WebSocket, embedded SPA, browser launch, lifecycle | HTTP |

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
always the implicit admin (`local-admin`); server mode resolves from session /
Bearer auth (wired in #14, until then `401`). Two shared helpers enforce the
ownership model (ADR 0007) in one place: `scope(owner_col, user)` (the
`owner == caller OR owner IS NULL` read filter) and `assert_admin(user)`. The
`/api/whoami` route exposes the resolved caller to the SPA.

## MCP endpoint (ADR 0008)

`server/routes/mcp.rs` is a hand-rolled JSON-RPC 2.0 endpoint at `POST /mcp`
(protocol `2025-03-26`), no MCP server crate. It is transport/dispatch plumbing:
`initialize` (serverInfo + capabilities + instructions), `tools/list`,
`tools/call`, and the `notifications/initialized` ack. A `ToolRegistry` holds
`Tool`s (name + JSON input schema + async handler); each handler returns a
`ToolOutcome` the dispatcher wraps into the MCP `content`/`isError` envelope.
Unknown method → `-32601`, unknown tool → `-32602`. A built-in `echo` stub
proves dispatch; the Epic 9 services (engine #27, DB #28, interactive #33)
register their real tools into the registry.

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
(`CurrentUser`) WebSocket. It spawns the engine configured on `AppState.engine`
(set from `--engine` / `CHESS_BASE_ENGINE`; absent ⇒ `503`) for the socket's
lifetime and `select!`s between client control messages
(`{"type":"analyse",…}` / `{"type":"stop"}`) and streamed engine events, restarting
cleanly (stop → drain → re-`go`) when a new position arrives mid-search. A real
engine is integration-tested behind `CHESS_BASE_TEST_ENGINE` (skipped if unset).

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

Indices cover `zobrist`, the games header columns (`database_id`, player/event FKs,
`date`, `eco`, `result`) and `database_id`/`owner_id` scoping. Migration `m0002_core_schema`
adds all of the above and runs on both SQLite and Postgres.

Planned (feature epics): `users`/auth and MCP/AI-assistant tables.

### Position search

Every indexed position is keyed by the 64-bit Polyglot-compatible Zobrist hash
produced by `position::zobrist_of_fen`. The same scheme works identically on
SQLite and Postgres (a plain indexed integer column), avoiding a separate
key-value store while covering the self-hosted scale we target.

## Build & CI

`make build` builds the SPA then `cargo build --release`. CI
(`.github/workflows/ci.yml`) builds the frontend, runs frontend tests, then
rustfmt + clippy (`-D warnings`) + cargo build + tests.

## Roadmap

See the epics in `.claude/CLAUDE.md`. Each epic is a GitHub milestone; concrete
features are individual issues. Epic 7 adds an MCP JSON-RPC endpoint (mirroring
the `site` project's `routes/mcp.rs`) exposing a transport-agnostic `StudyService`
so an external AI client — or, later, an embedded Claude assistant — can build and
annotate studies.

**Epic 9 — LLM study generation pipeline** is the AI-studies design proper
(see [ADR-0009](decisions/0009-llm-study-pipeline.md)): the LLM is an *annotator*,
with the engine and database as the sole source of chess truth. Code-orchestrated
preprocessing *stages* (variation-tree builder; a pawn-structure/key-square
**feature extractor** — the project's center of gravity) produce a finished, tagged
tree; the LLM annotates it; every concrete claim is verified against engine/DB
before commit. Engine eval/PV never enters the model context in batch mode. The
engine/DB service is exposed both as MCP tools (interactive) and as direct
in-process calls (batch).
