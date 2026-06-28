# chess-base — Project Brief

Self-hosted **ChessBase replacement**: a Rust backend + Vue 3 frontend to collect,
store, search and study chess games, with engine analysis and AI-assisted studies.

## Run modes (dual)

One binary, two modes (see `src/server/config.rs`, `src/db/config.rs`):

- **Local** — embedded **SQLite**, single user (implicitly admin), **auto-opens the
  browser**. `cargo run` / `make run`.
- **Server** — **Postgres**, multi-user. `chess-base --server --database-url postgres://…`.

## Architecture (single crate + frontend)

Not a workspace — one crate with modules, frontend embedded via `rust-embed`.
Full detail in [`../docs/architecture.md`](../docs/architecture.md); decisions in
[`../docs/decisions/`](../docs/decisions/).

```
src/
  position.rs      pure: FEN/SAN, legal moves, Zobrist hash (shakmaty)   ← unit-tested
  pgn_tree.rs      pure: study move-tree (variations/comments/NAGs)       ← unit-tested
  openings.rs      pure: ECO classification (embedded lichess dataset)     ← unit-tested
  db/              SeaORM: config (SQLite/Postgres), entities, migrations
  ingest.rs        ingest_pgn: parse PGN → store game → replay → position_index   ← shared by collectors
  collectors/      GameSource trait + Lichess / Chess.com adapters
  engine.rs        UCI engine config + parsing (Stockfish, Lc0/Maia)
  ai/llm/          LlmProvider trait + Anthropic Messages API client (Transport seam, key server-side)
  auth/            server-mode auth: users/sessions, Argon2, AuthService (ADR 0015)
  server/          Axum app: routes, state, embedded SPA, browser launch,
                   MCP /mcp + its auth (OAuth 2.1 / service token, ADR 0016)
  bin/chess-base.rs  CLI entry (clap)
frontend/          Vue 3 + Vite + Pinia + Tailwind v4 + chessground
```

**Layering rule:** pure logic (`position`, `pgn_tree`, `openings`) is I/O-free and fully
unit-testable; `db`/`collectors`/`engine`/`server` are thin, DI'd adapters. Keep new
tool/business logic in transport-agnostic services so HTTP **and** the planned MCP
endpoint are both thin callers.

## Commands (use the Makefile)

- `make build` — build frontend then release binary (embeds SPA).
- `make run` — local mode, opens browser.
- `make dev` — backend on `:3030` + Vite hot-reload (proxies `/api`).
- `make test` — Rust unit + integration + frontend tests.
- `make coverage` — `cargo llvm-cov` + vitest coverage.
- `make lint` — clippy (`-D warnings`) + `cargo fmt --check` + eslint.

Always `nvm use` (Node 22, see `frontend/.nvmrc`) before raw npm commands;
set `CARGO_BUILD_JOBS=4` for cargo (the Makefile does both).

## Engineering standards (project-specific)

- **File cap: 500 lines.** Split along a natural seam before crossing it.
- **Coverage target ~40–60%**, measured by `make coverage`.
- **Testable-first**: pure logic in `position`/`pgn_tree`; adapters injected.
- **Tests ship with the change.** Backend changes carry unit + integration tests.
- **Record decisions** as a short ADR in `../docs/decisions/` when you make an
  architectural choice; keep this brief and the architecture doc in sync.
- Rust: no `unwrap()`/`expect()`/panics on any I/O / input / DB / network path —
  propagate with `?` + `anyhow` context; never leak raw `DbErr` to clients.

## Data model essentials

A **database** (`databases` table) is a first-class, ownable collection of games:
`owner_id` NULL ⇒ a **global** (admin-managed) database searchable by every user;
otherwise it belongs to that user. Search scope = caller's databases ∪ global ones.
**Position search** keys on the 64-bit Zobrist hash from `position.rs`.

## Roadmap (epics → GitHub milestones)

0 scaffold (this) · 1 core domain & DB · 2 collection (Lichess/Chess.com/master) ·
3 search (header + position) · 4 studies UI · 5 engine analysis (auto-download
Stockfish/Lc0/Maia) · 6 auth/settings + roles · **7 MCP / AI-assisted studies**
(JSON-RPC `/mcp` endpoint mirroring the `site` project; `StudyService` tools) ·
8 packaging & deployment (local release binaries; server Docker + Postgres) ·
**9 LLM study generation pipeline** — the AI-studies design (ADR-0009): LLM as
annotator, engine/DB as ground truth, preprocessing stages + verification loop.
