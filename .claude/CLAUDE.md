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
  pgn_tree.rs      pure: study move-tree (variations/comments/NAGs/shapes/[%eval] +
                   set-up start_fen, ADR-0028: [FEN] header honoured on import/export) ← unit-tested
  openings.rs      pure: ECO classification (embedded lichess dataset)     ← unit-tested
  plans.rs         pure: engine-PV → per-piece trajectories (ADR 0017)      ← unit-tested
  features.rs      pure: position feature tags (material/phase/check, #33)    ← unit-tested
  threats/         pure: hanging-piece scan → red threat arrows (#123); GET /api/threats ← unit-tested
  db/              SeaORM: config (SQLite/Postgres), entities, migrations
  ingest.rs        ingest_pgn: parse PGN → store game → replay → position_index   ← shared by collectors
  collectors/      GameSource trait + Lichess / Chess.com adapters
  engine.rs        UCI engine config + parsing (Stockfish, Lc0/Maia); analyse_multi (top-N MultiPV)
  review/          Mode A (#119): engine-only full-game review — classify (pure
                   buckets + accuracy), explain (pure MoveFact "why" + the seam to
                   Mode B), service.review_game, POST /api/games/{id}/analyse   ← unit-tested
  games/export.rs  pure: mainline → MoveTree (+#119 review: [%eval]/NAGs/why) for
                   GET /api/games/{id}/export?annotated= — extended-PGN download (#120) ← unit-tested
  ai/llm/          LlmProvider trait + Anthropic Messages API client (Transport seam, key server-side)
  ai/providers.rs  ProviderService over llm_providers table (#20): admin-managed providers
                   (key server-side); default row builds the provider at startup, else env
  ai/assistant/    embedded Claude study assistant (#20, Direction B): agent loop driving the
                   SAME in-process MCP ToolRegistry — iteration cap + per-tool approval
                   (mutating tools gated); store.rs persists sessions/transcript   ← unit-tested
  study_gen/       study-gen stages (Epic 9): tree (#29) builds a pruned VariationTree
                   (TreeConfig.max_children_by_depth tapers branching with depth —
                   broad near the root, narrow on deep main lines, #160);
                   features.rs (#30) pure pawn-structure & key-square concepts;
                   annotate.rs (#31) batch LLM annotation pass + verification loop
                   (tool-free prompt, claims checked vs engine/DB before commit);
                   generate.rs (#115) orchestrator: tree → annotate/verify → persist a
                   study; exposed via POST /api/studies/generate (NOT MCP, ADR-0027);
                   danger.rs (#131, ADR-0026) pure "danger-map" classifier — trap
                   weapon/hope-chess + only-move gap (engine as adjudicator);
                   spine.rs (#139) PGN-repertoire walk: per opponent position runs
                   analyse_multi (movetime/variation) → reachability/trap/only-move
                   /attack → a tagged DangerTree (Weapon/Caution/Off-book);
                   attack.rs (#142) pure pawn-storm-toward-king detector reusing
                   plans.rs → Caution;
                   danger_generate.rs (#140) orchestrator: spine walk → fold to a
                   VariationTree (role tags as concept hints) → annotate/verify →
                   persist a study; surfaces rejected claims + role tags;
                   exposed via POST /api/studies/generate-danger-map (#141, NOT
                   MCP, ADR-0027; studies/danger_route.rs). The engine-only
                   sibling POST /api/studies/danger-map (#156, same file) is a
                   thin caller over walk_danger_spine_live returning the raw
                   DangerTree (+roles digest) — NO LLM, so the FE danger overlay
                   works on a no-key install;
                   seed.rs (#155) LLM-free seed seam: convert a built tree to a
                   MoveTree (move_tree_from, carries start_fen) → create_with_tree;
                   backs the data tools' `save_as` (no LLM, no PGN round-trip)  ← unit-tested
  auth/            server-mode auth: users/sessions, Argon2, AuthService (ADR 0015)
  server/          Axum app: routes, state, embedded SPA, browser launch,
                   MCP /mcp + its auth (OAuth 2.1 / service token, ADR 0016).
                   routes/mcp/ tools: engine_analyse + analyse_position/analyse_game
                   (#125), study_* (create/get/import_pgn/add_move/annotate/export,
                   #125), list_databases/db_list_games/db_read_game (#125),
                   preprocess.rs data tools opening_tree/danger_map/position_concepts
                   (ADR-0027, no internal LLM); opening_tree/danger_map take an
                   optional `save_as` to seed a study server-side (#155, study_gen::seed,
                   returns {study_id,node_count}, no tree JSON) — all thin callers of
                   the shared services.
                   routes/assistant.rs: AI assistant chat + provider registry (#20)
  bin/chess-base.rs  CLI entry (clap)
frontend/          Vue 3 + TypeScript + Vite + Pinia + Tailwind v4 + chessground
                   (strict `vue-tsc`; shared API/domain types in src/types.ts; ADR 0021)
```

**Layering rule:** pure logic (`position`, `pgn_tree`, `openings`, `plans`) is I/O-free and fully
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

- **KISS.** Most direct expression; no premature abstraction or indirection where a plain function works.
- **DRY.** Extract shared logic on the second occurrence — never copy a helper into a third file.
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
