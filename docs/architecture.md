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
| `pgn_tree` | Study move-tree: variations, comments, NAGs, and pinned board `Shape`s (arrows/highlights mirroring chessground, `set_shapes`, issue #61 — `serde(default)` so pre-#61 `tree_json` still loads, no migration); `pgn` submodule streams standard PGN ⇄ `MoveTree` (`from_pgn`/`to_pgn`, SAN validated via `position`; a set-up `[SetUp]`/`[FEN]` header is honoured on import and re-emitted on export, recorded as `MoveTree.start_fen` — `serde(default)`/skipped so standard-start `tree_json` is unchanged, no migration — so a study can start from a custom position and round-trip, ADR-0028 / issue #135); `shapes` submodule codes pinned shapes as Lichess `[%csl]`/`[%cal]` comment commands (round-tripped by `pgn`); `eval` submodule codes a per-node engine evaluation (`Node.eval`, White's perspective) as a `[%eval]` comment command (issue #120, also round-tripped by `pgn`; `serde(default)`/skipped so pre-#120 `tree_json` still loads, no migration); `lichess` submodule wraps the annotated movetext in PGN header tags for a Lichess-study chapter (`to_lichess_study`, issue #32; `pgn::push_header_tag` is the shared tag writer) | none (pure) |
| `openings` | ECO classification: embedded lichess `chess-openings` dataset → O(1) `zobrist -> (eco, name)` lookup; classifies a game by the longest match along its mainline (`eco_of_position`, `classify_mainline`) | none (pure) |
| `plans` | Engine-PV → per-piece trajectories (`plan_from_pv`, ADR 0017): traces only the start FEN's side-to-move, chaining moves by square continuity into `Trajectory{piece,squares}` paths (`g1→f3→g5`); opponent replies applied but not traced; `max_moves`-capped, panic-free | none (pure) |
| `features` | Factual position **feature tags** (`features_of_fen`, #33): material census + balance, game phase, side to move, check/mate/stalemate, insufficient material, mobility, castling rights, plus a short human-readable `tags` list; grounded facts the interactive analysis tool hands the model (deeper pawn-structure classification #30 layers on) | none (pure) |
| `threats` | Static **threat scan** for the Threats board overlay (`threats`, #123): which of the side-to-move's pieces are hanging — attacked by the opponent and either undefended or defended only behind a cheaper attacker — surfaced as red `threat` arrows (shared `pgn_tree::Shape`) from the cheapest attacker to each target. A cheap, deterministic attack/defence scan (no engine search; ignores pins/X-rays/deeper tactics by design). HTTP route (`threats/routes.rs`, `GET /api/threats?fen=…` → JSON `Shape[]`) is a thin caller | none (pure) |
| `db` | SeaORM connection, entities, migrations; SQLite/Postgres selection | DB |
| `databases` | Transport-agnostic `DatabaseService`: collection CRUD (create/list/get/rename/delete) over the `databases` table; `kind ∈ {lichess,chesscom,master,own}`, `index_depth` derived from `kind`. `list_with_counts` pairs each visible database with its game count in one grouped query (powers the MCP `list_databases` tool, #125). Ownership read scope + write guards (ADR 0007/0011) — global (`owner_id IS NULL`) create/mutate requires admin. HTTP routes (`databases/routes.rs`, `/api/databases`) are thin callers | DB |
| `studies` | Transport-agnostic `StudyService`: study lifecycle CRUD (`create`/`list`/`get`/`rename`/`delete`) + PGN import/export (`import_pgn`/`export_pgn` via `pgn_tree::pgn`, issue #9 — `export_pgn(include_eval)` keeps or strips the per-move `[%eval]` annotations for the extended vs plain export, issue #120; `export_lichess` emits a header-tagged Lichess-study chapter, issue #32; both export routes serve a real `.pgn` download via `server::download::pgn_attachment`) + node-level `MoveTree` mutations (`add_move`/variation SAN-validated via `position::legal_sans`, `annotate`, `promote_variation`/`reorder_variation`/`delete_node`, issue #18; `set_shapes` pins a plan's board shapes to a node, issue #61); ownership read scope + write guards (ADR 0007/0011). Every replay (`fen_at`, edits, export) seeds from the tree's `start_position()` (`start_fen` or the standard start), so a study imported from a set-up `[FEN]` edits and renders from that origin (ADR-0028). Pure of HTTP/MCP — both the HTTP routes (`studies/routes.rs`, `/api/studies`) and the scoped MCP study tools (`server/routes/mcp/study_tools.rs`, issue #17/#125 / ADR-0016) are thin callers | DB |
| `settings` | Transport-agnostic `SettingsService`: per-user UI preferences (theme, board theme, piece set, default database, plus the board-overlay layer toggles `show_plans`/`show_threats`/`show_master_moves`, #123) stored as one JSON blob per user under a `user_settings:{id}` key in the key/value `settings` table — no new entity. Validates the theme value and that `default_database_id` is visible to the caller (own ∪ global). HTTP routes (`settings/routes.rs`, `GET/PUT /api/settings`) are thin callers | DB |
| `auth` | Server-mode auth (ADR 0015): `users`/`sessions` tables, Argon2 hashing, transport-agnostic `AuthService` (register/login/logout/authenticate), `/api/auth/*` routes. Inert in local mode | DB |
| `ingest` | Shared game-ingest path (`ingest_pgn`): parses a PGN, dedups players/event, stores the game, replays the mainline via `position::replay`, and bulk-inserts the `position_index` rows (one per ply, capped by the database's `index_depth`; ADR-0003). One transaction per game; every collector funnels through it. A game carrying a provider permalink (`source_ref`) already present in the target database is skipped (returns `Ok(None)`), so a re-sync never doubles games (issue #95). `ingest_pgn_all` ingests every game in a multi-game blob (splitting on `[Event ` — the helper shared with the streaming collectors), returning only the newly-stored games; used by PGN upload. The store path is factored into reusable seams — `parse_pgn`, `prepare_game` (validate/replay), `load_index_depth` and `store_prepared` (write one game + its index rows in a caller-supplied transaction) — so the bulk importer can batch many games per transaction. `ParsedGame::content_hash` is a SHA-256 dedup key (stored as `source_ref`) for permalink-less master games | DB |
| `search` | Transport-agnostic search services. `PositionSearchService` (ADR-0003): "find games reaching this position" (`games_with_position`) and the opening tree of aggregated per-continuation stats (`opening_tree`: count + W/D/L), both keyed on the Zobrist `position_index`. `HeaderSearchService` (issue #6, `search/headers.rs`): query games by player/color/event/ECO-prefix/date-range/result, keyset-paginated on a stable `(sort, id)` cursor. Both scope to own ∪ global databases via `databases.owner_id`. The `report` submodule (`PositionReportService`, #28) layers the **pre-chewed** query surface on top of position search — reusing `opening_tree`/`games_with_position` and adding ECO (`openings`), per-move frequency/score and transpositions (distinct move orders reaching a Zobrist) — exposed as internal batch functions and the MCP DB tools. HTTP routes (`search/routes.rs`): `GET /api/search/{tree,games}` stream NDJSON, `GET /api/search/headers` returns a `{ games, next_cursor }` JSON page; thin callers of the services. The SPA's search surface is `SearchView` (issue #69, see "Search UI") | DB |
| `games` | Transport-agnostic `GameService` (issue #68): offset-paginated, sortable `list` of the games in a database (`GameSummary` rows; `GameListParams` = page/clamped-`limit`/`GameSort`/`SortDir`, default date-desc = newest-first, `id` tiebreaker; `GamePage` carries `total` for a real paginator) and single-game `get` (`GameDetail` with PGN movetext + `variant`/`start_fen` for board playback). Visibility follows ownership (own ∪ global). The pure `export` submodule (issue #120) turns a game's mainline — optionally with the #119 review — into a `MoveTree` the shared `pgn_tree::pgn` serializer renders (`linear_tree`/`annotated_tree`: `[%eval]` on every ply, a move-quality NAG + the rule-based why-note on the faults; `to_annotated_pgn` adds header tags). HTTP routes (`games/routes.rs`, `GET /api/games?database_id=…&page=…&limit=…&sort=…&dir=…`, `GET /api/games/{id}`, `GET /api/games/{id}/tree` — the stored PGN parsed into a `MoveTree` via `pgn_tree::pgn::from_pgn_with_start`, preserving the `(…)` variations chess.js drops, with the game's `start_fen` threaded so a SetUp origin round-trips (#135), and `GET /api/games/{id}/export?annotated=&depth=` — a real `.pgn` download, verbatim or engine-annotated) are thin callers | DB |
| `collectors` | `GameSource` trait + Lichess / Chess.com adapters and the shared `SyncCursor`/`SyncOutcome`. Each adapter's `sync()` streams the provider export and funnels every game through `ingest`: Lichess exports user games as one PGN stream (incremental via `since`, resumed *at* the last game's second); Chess.com lists monthly archives then fetches each month's PGN (incremental via the month cursor, re-syncing the cursor month). Both resume from the boundary and rely on `ingest`'s `source_ref` dedup to avoid doubling (issue #95). The `bulk` submodule (`BulkImporter`, issue #4) streams a (optionally `.zst`-compressed) master PGN file in bounded memory — chunked reads, drained game-by-game on `[Event ` boundaries — committing in batched transactions (`store_prepared`) with SQLite bulk PRAGMAs (WAL, `synchronous=NORMAL`, …) applied first. Games are deduped by `ParsedGame::content_hash` (stored as `source_ref`, unique per database), so a re-run skips everything already imported (restartable); `find_or_create_master` resolves the target global `master` database. Driven by the `chess-base import-pgn <FILE>` CLI subcommand. All boundary/back-off/cursor decisions are pure, unit-tested helpers | HTTP / CLI |
| `imports` | Transport-agnostic `ImportService` (issue #70): trigger a provider `sync` (Lichess/Chess.com) or ingest an uploaded PGN (`import_pgn` → `ingest_pgn_all`) into a target database, behind the same write guard as `databases` (ADR 0007/0011). A `sync` loads the cursor persisted per `(database, source)` (`imports/cursor.rs` ⇄ `sync_cursors`), runs the collector from it and saves the advanced cursor, so re-syncs fetch only new games (issue #95). HTTP routes (`imports/routes.rs`): `POST /api/import/sync` and `POST /api/import/pgn`, both returning `{ imported }`; thin callers. The SPA surface is `ImportView` (`/import`) | DB / HTTP |
| `engine` | UCI engine config + message parsing (`command`/`analysis` pure), the `manager::Engine` process manager (spawn, handshake, `setoption`, `position`/`go`/`stop`, streamed analysis), the pooled `service::EngineService` facade — one-shot `analyse` plus `analyse_multi` (top-N MultiPV lines for the game-review pass, #119) for batch + MCP (ADR 0014) — and the `download` auto-download manager (platform catalog → fetch + checksum + register, #11) (Stockfish, Lc0/Maia) | process / HTTP |
| `review` | Fast **engine-only full-game review** (Mode A, #119): replay a stored game and search every position, classifying each ply and writing a rule-based "why" note — no LLM, no API key. `classify` (pure): win-probability buckets (best / great / good / inaccuracy / mistake / blunder) from the eval before vs after a move, plus per-side ACPL + accuracy roll-up (`summarize`). `explain` (pure): the reusable `MoveFact` struct (eval delta, engine's preferred move/line, material won/lost resolved over the PV, missed/allowed mate) rendered as a terse note (`explain`) — the **seam** Mode B's LLM annotation (#31) is meant to consume as ground truth so its strategic prose stays engine-grounded. `service::review_game` is the thin engine shell (gathers one `analyse_multi` per position, then the pure `assemble` does all classification/explanation); `routes` exposes `POST /api/games/{id}/analyse?depth=` returning per-ply `{eval_cp, mate, best_move, best_line, played_rank, classification, explanation}` (`best_line`: the engine PV in SAN, ≤6 plies, for grafting a variation at a critical position, #135) + a per-side summary. Shares the one-shot engine pool (not the interactive WS, so it never starves live analysis) | none (pure core) + engine adapter |
| `ai/llm` | Provider-agnostic LLM client: `LlmProvider` trait + Anthropic Messages API client (ADR 0013); HTTP behind an injectable `Transport` seam | HTTP |
| `ai/providers` | `ProviderService` over the `llm_providers` table (#20, ADR-0025): admin-managed providers (`list`/`upsert`/`delete`, keys **write-only** — the `ProviderInfo` DTO omits them). `resolve` builds the active `Arc<dyn LlmProvider>` from the default DB row, else the `ANTHROPIC_API_KEY` env fallback — consumed at startup to fill `AppState.llm_provider` | DB |
| `ai/assistant` | Embedded Claude study assistant (#20, Direction B / ADR-0025): an in-app chat whose agent loop reuses the **same** in-process `ToolRegistry` the `/mcp` transport serves (no second tool impl). `service::AssistantService` drives the loop: ask the provider → run read-only tool calls automatically → **pause** on any *mutating* tool (`study_create`/`study_import_pgn`/`study_add_move`/`study_annotate`) until the user approves/denies → resume; bounded by `MAX_ITERATIONS` rounds per user message (both the cap and pending approvals are surfaced to the SPA). `store::AssistantStore` persists `assistant_sessions` + the `assistant_messages` transcript (one `ai::llm::Message` JSON per row); sessions are private (owner-scoped). Pure gating/view helpers (`requires_approval`, `pending_approvals`, `iterations_since_user`, `build_view`) are unit-tested; the loop is unit-tested with a stub provider + real in-memory store. HTTP routes (`server/routes/assistant.rs`, `/api/assistant/*`) are thin callers | DB + `ai/llm` provider + tool registry |
| `study_gen` | Study-generation pipeline stages (Epic 9 / ADR-0009): deterministic preprocessing (`tree`, `features`) feeding the LLM annotation pass (`annotate`). `tree` (#29): from a start FEN, breadth-first builds a bounded, pruned `VariationTree` of DB-played continuations, each node tagged with an engine eval + the pre-chewed DB stats; pruning (`select_continuations`) drops moves below a frequency floor or outside an eval margin, capped by `max_children`/`max_depth`/`max_nodes` (`TreeConfig`). Tree types, pruning and the BFS walk are I/O-free over two seams (`Evaluator`, `ContinuationSource`); the concrete engine/DB adapters (`EngineEvaluator`, `ReportContinuations`) live in the module root. `features` (#30, `features.rs` + `features/derived.rs`): pure pawn-structure & key-square classifier — `concepts_of_fen` pattern-matches the pawn skeleton into structure tags (IQP, hanging pawns, Carlsbad, hedgehog, Maroczy, Stonewall, French chain), key squares (blockade / bind / outpost / break / chain-base, each with beneficiary), open/half-open files, isolated/doubled/passed/backward pawns, a king-safety signal and material imbalance (bishop pair, opposite-coloured bishops); the builder attaches the resulting `Concepts` to every node. `annotate` (#31): **batch** LLM annotation pass over the finished tagged tree → comments + NAG glyphs + training questions. `build_prompt` feeds the model only the moves, concept tags and opening name — **no tools, and no engine eval / PV / DB stats in the context** (ADR-0009). The model's draft attaches machine-checkable `Claim`s (`only_move`, `best_move`, `blunder`, `loses_material`/`wins_material`), and `verify_and_commit` confirms each against ground truth (legal-move legality + the tree's stored engine eval) before committing into a `MoveTree`; a claim ground truth contradicts is dropped, taking the prose that rested on it with it (`Rejection` records what fell). Pure verification (stored eval + chess rules), so the whole loop is unit-tested with a stub provider and no engine. `generate` (#115): the **orchestrator** that ties the three stages into one user-invokable operation — `generate_study` runs the tree builder → batch annotation/verification pass → persists the verified `MoveTree` as a `studies` row owned by the caller (`StudyService::create_with_tree`), returning the new study id + a summary (node count, rejected-claim count). Generic over the `Evaluator`/`ContinuationSource`/`LlmProvider` seams (unit-tested with injected fakes + a real in-memory study service); `generate_study_live` is the production wrapper. Exposed through `POST /api/studies/generate` — it is **not** an MCP tool (ADR-0027: the MCP surface exposes the deterministic `tree`/`features` stages as data via `opening_tree` / `position_concepts`, and the LLM that annotates them runs on the client side of the MCP boundary). **Danger-map mode** (ADR-0026, a second mode beside the best-line builder): `danger` (#131) is the pure classifier — given perspective-normalised centipawn evals it returns the trap verdict (`Weapon`/`HopeChess`/`Quiet`) and only-move gap; `attack` (#142) reuses the `plans.rs` PV tracer to detect a pawn storm marching toward the enemy king; `spine` (#139) is the driver — it walks a PGN repertoire (the "spine") from move 0 and, at every searched opponent position, folds `analyse_multi` + DB stats through those signals into a tagged `DangerTree` whose nodes carry **Weapon** / **Caution** (refuted bait *or* an attack our move concedes) / **Off-book** roles. The walk depends only on the `MultiAnalyzer` + `ContinuationSource` seams (unit-tested against fakes). `danger_generate` (#140) is the mode's orchestrator: it folds the tagged `DangerTree` into a `VariationTree` (each node's role becomes a synthetic concept hint, `eval` left `None`), runs the shared annotate/verify pass, and persists a study — surfacing the rejected claims and role tags. Exposed through `POST /api/studies/generate-danger-map` (`studies/danger_route.rs`, #141) — **not** an MCP tool (ADR-0027: the `spine` walk is exposed as ground-truth data via the `danger_map` MCP tool instead, with annotation done client-side); the request carries the spine as PGN, a per-variation `movetime_ms`/`multipv` budget, and partial `SpineConfig`/`DangerConfig`/`AttackConfig` overrides (all `serde(default)`) | none (pure core) + engine/DB adapters + `ai/llm` provider |
| `server` | Axum router, app state, request identity, MCP `/mcp` endpoint + its auth (OAuth 2.1 / service token, ADR 0016), engine analysis WebSocket, embedded SPA, browser launch, lifecycle | HTTP |

The **pure** modules (`position`, `pgn_tree`, `openings`, `plans`) carry the chess logic and are unit-tested without any
runtime. Everything else is a thin adapter with dependencies injected, so the
business logic stays testable and reusable across transports (HTTP and the MCP
`/mcp` endpoint).

### Frontend (`frontend/`)

Vue 3 + **TypeScript** + Vite + Pinia + Tailwind v4. Board rendering via
**chessground**; client-side move legality via **chess.js**. Built to
`frontend/dist` and embedded into the binary with `rust-embed`
(`src/server/embed.rs`). `build.rs` guarantees the folder exists so the crate
always compiles even before the SPA is built.

The SPA is strictly typed: `vue-tsc -b` runs in `npm run build` and `npm run lint`
(so both CI and `make lint` gate on it). Shared API/domain types live in one
module, `src/types.ts`, imported by the typed `api.ts` client, the Pinia stores
and the SFCs; see ADR 0021.

`App.vue` is a thin nav/layout shell around a `<router-view>`; **vue-router**
(`router/index.ts`, HTML5 history) maps each top-level surface to a lazily-loaded
view in `views/`: `AnalysisView` (`/`, the board + analysis panel + a
variation-tree notation panel — `components/MoveTree.vue`, click a move or
←/→/Home/End to jump the board & engine across the tree; playing off the line
branches a variation, issue #121), `GamesView`
(`/games`, the game browser), `StudyView` (`/studies`, the variation-tree editor,
see "Study editor" below), `ImportView` (`/import`, the game-import UI — see
"Game import" below), `SearchView` (`/search`, see "Search UI" below) and
`LoginView` (`/login`, the server-mode register/login form, see "Auth UI" below)
plus `CollectionsView` (`/collections`, the collections manager — create/list/rename/delete
databases via `/api/databases`, store `stores/collections.ts`). Deep links work because the server's
`static_handler` falls back to `index.html` for unknown paths
(`src/server/routes/mod.rs`).

**Shared board building blocks** (issue #134) keep the three board pages —
Analyse, Study, Game review — on the same tooling: `lib/useTreeBoard.ts` is the
chess.js-backed tree/cursor state machine (`tree`/`currentId`/`fen`/`legalDests`
+ `seek`/`playMove`/`goto`/`undo`/`load`…); `lib/useBoardOverlays.ts` drives the
position-derived overlay layers off a `() => fen` getter and composes them via
`lib/boardShapes.ts`; `components/BoardControls.vue` is the nav row + overlay
toggles; `components/EnginePanel.vue` is the shared eval/PV display (eval bar +
PV lines + analyse toggle) driven by a `:fen` prop, with `#controls`/`#line-action`
slots for per-page extras.

State lives in Pinia stores: `stores/game.ts` (the Analyse board — wraps
`useTreeBoard` for the chess.js-backed position, legal-move `dests`, client-side
`MoveTree` cursor where `goto`/`next`/`prev`/`first`/`last` move the board &
engine without mutating the tree, replaying a known move follows it while a new
move branches a variation, and `undo` prunes the current node's subtree (#121);
adds the play-vs-engine `mode`/`playColor`),
`stores/games.ts` (the game browser —
offset-paginated, sortable list for a selected database, backed by `/api/games`;
the opened game rides the shared `useTreeBoard` composable (#134), seeded from
`GET /api/games/{id}/tree` (#135) so PGN `(…)` variations are kept and off-line
moves branch instead of truncating, plus `mainlinePath`/`plyOf`/`nodeAtPly`
helpers that map the tree's mainline node ids to plies and a `graftReview` action
that splices the engine review's better lines onto the live tree via the pure,
unit-tested `lib/reviewTree.ts` — each inaccuracy/mistake/blunder's `best_line`
appended as a sibling variation off the position before the played move,
idempotently, comment + eval set on the first grafted node; #136),
`stores/review.ts` (the engine-only full-game review,
Mode A #119 — one `POST /api/games/{id}/analyse` result with a `byPly` index plus
a `currentMove` getter that maps the board cursor back to its mainline ply;
`GamesView`'s board column is the shared `Board` + `BoardControls` + `MoveTree` +
`EnginePanel`, and `components/GameReviewPanel.vue` holds the review UI: an
**"Analyze game"** button (gated on the `/api/health` `engine` flag), the
`EvalGraph.vue` eval sparkline (its current-ply mark driven by `plyOf`, clicks
navigating via `nodeAtPly`), a per-side accuracy/ACPL/blunder summary and a
why-note for the current move via the pure `lib/reviewFormat.ts`; **"Export PGN"** /
**"Export with analysis"** buttons call `games.exportPgn(annotated)` and trigger a
`.pgn` download via the pure `lib/download.ts`, issue #120), `stores/engine.ts` (the
`/api/engine/analyse` WebSocket — folds streamed `info`/`bestmove` events into
reactive eval/PV state, and `planline` frames into per-MultiPV `plans` plus a
derived `shapes` overlay for the board; the socket factory is injectable for
tests) and
`stores/settings.ts` (per-user UI preferences with a `localStorage` mirror for
instant load; see "User settings" below) and `stores/auth.ts` (server-mode
session: register/login/logout + the resolved caller; see "Auth UI" below). The
WebSocket protocol parsing/formatting is isolated in the pure, unit-tested
`lib/engineStream.ts` (and `lib/pv.ts` for UCI→SAN). The pure, unit-tested
`lib/plansToShapes.ts` maps the engine's per-piece `trajectories` into chessground
auto-shapes — one brush per line (`plan1…3` + dimmed variants), a chained arrow
per square pair, the hovered line full-opacity and the rest dimmed (issue #60,
ADR 0017). **Board overlays** (issue #123) are organised into three independently
toggleable layers, composed in one place by the pure `lib/boardShapes.ts`
(`composeBoardShapes` returns the union of the enabled layers; `overlayBrushes`
adds the red `threat` + violet `master` brushes): the engine **Plans** layer
(`stores/engine.ts` `shapes`), the **Threats** layer (`/api/threats` →
`stores/overlays.ts`, mapped via `shapesToDrawShapes`) and the **Database
master-moves** layer (`/api/search/tree` → `lib/masterShapes.ts`, arrows sized +
labelled by frequency). `lib/useBoardOverlays.ts` (used by `AnalysisView`) watches
the position + each layer's persisted toggle (`stores/settings.ts`
`showPlans`/`showThreats`/`showMasterMoves`), re-loads the position-derived layers
and clears a layer the moment it is switched off; the toggle row + a **Clear
arrows** control (clears the user's hand-drawn shapes, `Board.clearUserShapes`)
live in `components/BoardControls.vue`. `components/AnalysisPanel.vue` embeds the
shared `components/EnginePanel.vue` (+ `EvalBar.vue`) — eval bar, MultiPV lines,
depth/nps and the analyse toggle — and adds the engine options + play-vs-engine
controls + the per-line **Pin** seam via slots; hovering a PV row sets the store's
active line so its plan highlights and the others dim. `Board.vue` is presentational (it also
drives the variation-tree game viewer in `GamesView`), emits user moves, and renders
the plan overlay via `setAutoShapes` (auto-shapes, so a user's right-click
drawings survive), cleared on every position change — unless `persist-shapes` is
set (the study editor's pinned plans, issue #61).

The **study editor** (issue #8, `views/StudyView.vue`) builds and annotates
commented PGN trees. `stores/studies.ts` owns the open study (`current`, with its
`MoveTree`) and its lifecycle CRUD/import/export (**"Export PGN"** / **"Export with
eval"** header buttons call `studies.exportPgn(id, withEval)` and download a `.pgn`
via `lib/download.ts`, issue #120); `stores/studyEditor.ts` layers
the editing state on top — the selected node id, the chess.js position derived for
that node (FEN, legal `dests`, last move), and the mutations: a board drag is
turned into SAN and either navigates to the matching child or appends a new
move/variation (`addMove`), plus annotate/promote/reorder/delete and `setShapes`
(pin an engine plan's arrows to the current node, issue #61) — each calling
`api.studies` and re-rendering from the returned tree. The **"Generate study"**
button opens `components/GenerateStudyDialog.vue` (Mode B #119): a start position /
engine-depth / repertoire-framing (variation depth + breadth) form over
`POST /api/studies/generate` (`stores/studies.ts::generate`), surfacing the
verification summary (committed nodes, rejected claims) and gated on the
`/api/health` `llm` flag (a hint to set `ANTHROPIC_API_KEY` when absent). The
sibling **"Danger map"** button opens `components/GenerateDangerMapDialog.vue`
(ADR-0026 #131): a repertoire-spine (PGN) / side / walk-depth / `movetime_ms` /
`multipv` form over `POST /api/studies/generate-danger-map`
(`stores/studies.ts::generateDangerMap`), surfacing the same committed/rejected
counts plus the engine-tagged danger roles (Weapon / Caution / Off-book). Pinned shapes are drawn on
`Board.vue` (chessground `autoShapes`, with `persist-shapes` so a node's plan
survives navigation); `AnalysisPanel.vue`'s per-line **Pin** button converts that
line's plan trajectories (`lib/plansToShapes.ts`) into stored `Shape`s. The pure tree walking
(path/line to a node, child lookup, and flattening the tree into renderable
move/variation tokens with move numbers + NAG glyphs) is the unit-tested
`lib/moveTree.ts`. `components/MoveTree.vue` renders those tokens (click to
navigate, variations dimmed by depth) and drives **both** the study editor and the
analysis board; `components/AnnotationEditor.vue` edits the selected node's
comment/NAG and promotes/deletes variations (study editor only). The analysis
board feeds `MoveTree.vue` from the `game` store's client-side tree — the pure
mutators (`emptyTree`/`appendChild`/`deleteSubtree`) live alongside the read
helpers in `lib/moveTree.ts`.

In dev, Vite serves the SPA and proxies `/api` (with `ws: true` for the engine
WebSocket) to the backend on `:3030`.

## Run modes

`Mode::Local` → SQLite + auto-open browser + single implicit admin user.
`Mode::Server` → Postgres + multi-user. Selected in `src/bin/chess-base.rs` from
CLI flags; resolved into `AppConfig` (config) → `AppState` (runtime). The binary
also exposes an `import-pgn <FILE>` subcommand that runs the `collectors::bulk`
master importer and exits without serving (issue #4).

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
shows no login controls. `api.ts` keeps the session token in memory plus a
`localStorage` mirror (`setAuthToken`/`getAuthToken`) and attaches it as
`Authorization: Bearer <token>` to every request (the HttpOnly `session` cookie
the backend sets still works too — the Bearer header just lets the client decide
when it authenticates and drop it on logout). `stores/auth.ts` resolves the run
mode from `/api/health` once (`init()`), restores the user via `/api/whoami` when
a token is present, and exposes `register`/`login`/`logout` plus `needsAuth`. The
router guard (`authRedirect` in `router/index.ts`) bounces gated navigations to
`LoginView` (`/login`) with a `redirect` query and sends already-authenticated
callers away from it. Backend error messages are pre-sanitized (generic for 5xx),
so the form surfaces them verbatim without leaking internals.

## MCP endpoint (ADR 0008)

`server/routes/mcp/mod.rs` is a hand-rolled JSON-RPC 2.0 endpoint at `POST /mcp`
(protocol `2025-03-26`), no MCP server crate. It is transport/dispatch plumbing:
`initialize` (serverInfo + capabilities + instructions), `tools/list`,
`tools/call`, and the `notifications/initialized` ack. A `ToolRegistry` holds
`Tool`s (name + JSON input schema + async handler); each handler returns a
`ToolOutcome` the dispatcher wraps into the MCP `content`/`isError` envelope.
Unknown method → `-32601`, unknown tool → `-32602`. The tool builders live in
`server/routes/mcp/tools.rs`: an `echo` stub proves dispatch and the engine facade
registers `engine_analyse` (#27, see ADR 0014). The **study-preprocessing tools**
live in `server/routes/mcp/preprocess.rs` (ADR-0027): `opening_tree` (the pruned,
eval/stats-tagged `VariationTree` from `study_gen::tree`), `danger_map` (the
engine-adjudicated `DangerTree` walked from a repertoire spine, `study_gen::spine`)
and `position_concepts` (the pure pawn-structure / key-square `Concepts`). They
return engine/DB ground-truth **data** with no language model *inside* the tool:
ADR-0027 puts the LLM that annotates them on the **client** side of the MCP
boundary (an external agent, or the embedded assistant driving this same
registry), so the old LLM-internal `generate_study` / `generate_danger_map` tools
are no longer on the MCP surface — their orchestrators stay reachable through
`POST /api/studies/generate{,-danger-map}`. The **study tools** live in
`server/routes/mcp/study_tools.rs` (#17, completed in #125): `study_create`,
`study_import_pgn` (build a whole study from PGN in one call, reusing the
`pgn_tree::pgn` parser), `study_get` (read back a study's `{summary, tree}` with
node ids — the seam that lets an agent annotate an existing study),
`study_add_move` (move as SAN **or** UCI — UCI sidesteps SAN's strict
disambiguation — returning `{node_id, fen, san}` and accepting an inline
`comment`/`nag`), `study_annotate` and `study_export`, all thin callers of
`StudyService` scoped to the caller. The **pre-chewed DB tools** live in
`server/routes/mcp/db_tools.rs` (#28, completed in #125): `db_position_report`
(ECO + per-move win/draw/loss with frequency/score + transpositions) and
`db_reference_games` (scoped reference games), thin callers of
`search::PositionReportService`; plus `list_databases` (the caller's + global
collections with game counts — how an agent discovers a `database_id`),
`db_list_games` (sortable offset page of a database's games) and `db_read_game` (one game
with its PGN), thin callers of `DatabaseService` / `GameService`, all returning
synthesized JSON the LLM consumes but never recomputes (ADR-0009). The
**interactive analysis tools** live in `server/routes/mcp/analysis.rs` (#33):
`analyse_position` is the one-shot "explain this position" entry point — it
bundles the engine eval/PV, the `db_position_report`, and the pure
`features::features_of_fen` feature tags (material, game phase, check/mate,
castling rights) into a single grounded snapshot so a connected client cites tool
output rather than inventing lines. A missing engine leaves `engine: null` with an
explanatory note; the DB report and features are always present. `analyse_game`
(#125) is its whole-game counterpart: it parses a PGN and walks the engine over
every ply via the #119 `review::review_game`, returning the per-ply review facts
(eval, best move, classification, note) + accuracy summary and the same game as
annotated PGN movetext (the single `games::export` + `pgn_tree::pgn` serializer,
#120). `analyse_position` and `engine_analyse` share one default search depth
(`engine::DEFAULT_DEPTH`). The unbundled tools stay available for an agent that
wants to drill in further.

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
`bundled-stockfish` build (issue #54, `engine/bundled.rs`) → an auto-downloaded
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

### Bundled Stockfish — optional embedded engine (ADR 0005 / #54)

`engine/bundled.rs` adds an **opt-in** `bundled-stockfish` Cargo feature (off by
default) for offline / air-gapped local builds. When on, `build.rs` checks the
per-target binary is present under `engines-bundled/<target>/` and **checksum-
verifies it at build time** (a mismatch fails the build; an absent binary fails
with guidance to run `make bundle-stockfish`), and `EngineAssets` (`rust-embed`,
mirroring the SPA embedding of ADR 0004) embeds it into the binary. At startup
`serve` calls `bundled::extract()`, which writes the embedded binary to the OS
cache dir (`dirs::cache_dir()/chess-base/engines-bundled/`), sets the executable
bit via a temp-file + atomic rename, and is idempotent (skipped when the on-disk
bytes already match). `bundled::bundled_engine()` is the pure resolution seam
`EngineRegistry::resolve_default` consults — the bundled build slots in below a
user override and above an auto-downloaded binary. The default build embeds
nothing (both functions are `None`/`Ok(None)` no-ops), so binary size is
unchanged and there is **no GPLv3 obligation**. The fetch step lives in
`make bundle-stockfish` (not `build.rs`) so the feature build stays
offline-capable and pulls no HTTP stack into the toolchain. **LICENSING:**
Stockfish is GPLv3, so a `bundled-stockfish` build artifact is GPLv3; the default
download build (a separately-fetched child process — mere aggregation) is
unaffected. Never bundles Lc0/Maia (large weights, per-target binary — download
only).

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
auth). The frontend `stores/settings.ts` Pinia store mirrors the server into
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
lazily, reuses idle ones, and caps live processes with a semaphore. Because that
pool is single-permit, user-supplied limits are bounded so one request can't pin
it (issue #93): `Limits::clamped` caps `depth`/`movetime_ms` (`MAX_DEPTH` = 60,
`MAX_MOVETIME_MS` = 30s) at the MCP arg boundary *and* inside `bounded`, and the
whole search runs under an overall deadline (movetime + grace, or a 60s ceiling)
so a stuck engine can't hang forever — on timeout the engine is discarded, not
reused. The streaming WebSocket keeps its own per-socket engine: it needs
incremental `info` updates and a mid-search `stop`, which the one-shot pool
deliberately does not model (it clamps client limits the same way). The
event-folding is pure and unit-tested; the live pool and MCP tool are
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
only**: it travels in the `x-api-key` header and never reaches the SPA. The server
builds the provider once at startup — the admin-configured default `llm_providers`
row (#20) wins, else the `ANTHROPIC_API_KEY` env fallback (`ai/providers::resolve`)
— and holds it on `AppState.llm_provider` (best-effort, like the engine: nothing
configured leaves it `None` and disables `generate_study` **and** the assistant).

## Embedded study assistant (Epic 7 / ADR-0025)

`ai/assistant` is the in-app chat counterpart to the `/mcp` transport: instead of
an external client, an embedded agent loop drives the **same** in-process
`ToolRegistry` (exposed via `ToolRegistry::tools`/`invoke`), so there is one tool
surface, not two. `AssistantService::post_message` appends the user turn and runs
`drive`: ask `LlmProvider::complete` (system prompt + the registry's `ToolSpec`s),
record the assistant turn, then — if it requested tools — run the read-only ones
automatically and **pause** if any is *mutating* (`requires_approval`). The pause
is just a trailing assistant turn whose tool calls have no results yet, so it is
resumable across requests: the SPA shows `pending_approvals`, the user approves or
denies per call, and `respond` runs the approved calls (denied → an error tool
result the model sees) and continues the loop. The loop is bounded by
`MAX_ITERATIONS` tool rounds since the last user message (`iterations_since_user`),
surfaced alongside the cap. Sessions are private, owner-scoped rows in
`assistant_sessions`; the transcript is `assistant_messages`, one
`ai::llm::Message` serialized per row. The HTTP surface is `/api/assistant/*`
(sessions CRUD, `messages`, `respond`, and the admin `providers` registry); a
`503` when no provider is configured. Direction-A (external MCP client)
deployments need none of this — the key never reaches the SPA either way.

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
  ply_count?, pgn?, source_ref?)` — one game with its PGN header roster. `variant`
  (default `standard`) + nullable `start_fen` make Chess960 / set-up positions
  first-class (ADR 0010); `date` is verbatim PGN text (may be partial, `1992.??.??`).
  `source_ref` is the stable provider permalink (Lichess `[Site]` URL / Chess.com
  `[Link]`), **unique per `database_id`** so a re-sync dedups instead of doubling
  (issue #95); `NULL` for games without a permalink (manual uploads), which never
  dedup.
- `position_index(id, zobrist, game_id, ply, move, database_id)` — one row per
  indexed mainline ply (ADR 0003); indexed on `zobrist` for "find games reaching
  this position". `database_id` is denormalized so search filters by scope without a
  join. The Zobrist `u64` is stored as `i64` by a **bit-preserving reinterpret**
  (`u64 as i64`, reversible — see `entities::position_index::{to_i64, from_i64}`),
  since neither backend has an unsigned 64-bit integer.
- `studies(id, database_id, owner_id?, name, tree_json, created_at)` — a named,
  serialized `pgn_tree::MoveTree` (JSON in `tree_json`); `owner_id IS NULL` mirrors
  the global-collection rule.
- `sync_cursors(id, database_id, source, last_month?, last_game_ms?)` — the
  persisted incremental-sync position, **one row per `(database_id, source)`**
  (issue #95). Archive sources (Chess.com) resume from `last_month` (`"YYYY/MM"`),
  stream sources (Lichess) from `last_game_ms`.
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
`date`, `eco`, `result`) and `database_id`/`owner_id` scoping, plus the two unique
indices from issue #95 (`games(database_id, source_ref)` and
`sync_cursors(database_id, source)`). Migration `m0001_init` seeds the `settings` + `databases`
tables; `m0002_core_schema` adds the rest of the core domain
(`players`/`events`/`games`/`position_index`/`studies`); `m0003_auth` adds `users`/`sessions`; `m0004_oauth` adds the MCP-auth tables
(`service_tokens`, `oauth_clients`, `oauth_codes`, `oauth_tokens`); `m0005_sync_dedup`
adds `games.source_ref` and the `sync_cursors` table; `m0006_assistant` adds the
embedded-assistant tables (`assistant_sessions`, `assistant_messages`,
`llm_providers`, #20). All run on both SQLite and Postgres.

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
driven by `stores/search.ts`. The pure, unit-tested logic is split out:
`lib/headerQuery.ts` owns the query state (empty shape, blank detection,
snake_case param mapping) and `lib/openingTree.ts` owns tree navigation (replay a
SAN line to a FEN + legal `dests` via chess.js, board-drag→SAN, stat math). The
store calls `api.search.{headers,tree,games}` — `headers` returns a JSON page,
`tree`/`games` parse the NDJSON streams.

### Game import (issue #70)

`ImportView` (`/import`) is the UI for the `imports` backend. A target-collection
picker (the caller's databases ∪ global) feeds two forms: a **provider sync**
(source = Lichess/Chess.com + username + optional Lichess token → `POST
/api/import/sync`) and a **PGN upload** (a `.pgn` file read in the browser →
`POST /api/import/pgn`). Each request runs as a tracked *job* in
`stores/import.ts`; the pure, exported `foldStatus(jobs)` folds the per-job
statuses into an overall summary (`idle`/`running`/`done`/`partial`/`error` +
total games imported) rendered as a status list. `foldStatus` and the store
actions are unit-tested, as is the view's form wiring. The store calls
`api.import.{sync,uploadPgn}`.

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
tools so an external AI client (Direction A) can read and analyse studies — and
an **embedded** Claude assistant (Direction B, #20 / ADR-0025) that reuses that
same in-process tool surface for an in-app chat (see "Embedded study assistant"). Study *authoring* — lifecycle CRUD + PGN import/export
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
