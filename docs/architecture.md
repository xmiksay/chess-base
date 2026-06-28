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
| `position` | FEN/SAN/UCI parsing, legal moves, move application & game replay, Zobrist hashing (shakmaty) | none (pure) |
| `pgn_tree` | Study move-tree: variations, comments, NAGs | none (pure) |
| `db` | SeaORM connection, entities, migrations; SQLite/Postgres selection | DB |
| `collectors` | `GameSource` trait + Lichess / Chess.com adapters, sync cursor | HTTP |
| `engine` | UCI engine config + message parsing (Stockfish, Lc0/Maia) | process |
| `server` | Axum router, app state, embedded SPA, browser launch, lifecycle | HTTP |

The two **pure** modules carry the chess logic and are unit-tested without any
runtime. Everything else is a thin adapter with dependencies injected, so the
business logic stays testable and reusable across transports (HTTP today, an MCP
endpoint next).

### Frontend (`frontend/`)

Vue 3 + Vite + Pinia + Tailwind v4. Board rendering via **chessground**;
client-side move legality via **chess.js**. Built to `frontend/dist` and embedded
into the binary with `rust-embed` (`src/server/embed.rs`). `build.rs` guarantees
the folder exists so the crate always compiles even before the SPA is built.

In dev, Vite serves the SPA and proxies `/api` to the backend on `:3030`.

## Run modes

`Mode::Local` → SQLite + auto-open browser + single implicit admin user.
`Mode::Server` → Postgres + multi-user. Selected in `src/bin/chess-base.rs` from
CLI flags; resolved into `AppConfig` (config) → `AppState` (runtime).

## Data model

- `settings(key, value)` — app/user key-value settings.
- `databases(id, owner_id?, name, kind)` — an ownable collection of games.
  `owner_id IS NULL` ⇒ a **global**, admin-managed database visible to all users;
  `kind ∈ {lichess, chesscom, master, own}`. Search scope for a user is *their*
  databases ∪ all global databases.

Planned (feature epics): `games`, `players`, `events`, a **position index**
`(zobrist, game_id, ply, move)` for "find games reaching this position", `studies`
(serialized `MoveTree`), `users`/auth, and MCP/AI-assistant tables.

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
