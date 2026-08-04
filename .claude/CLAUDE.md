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
                   set-up start_fen, ADR-0028: [FEN] header honoured on import/export);
                   graft_subtree(at, src) grafts a MoveTree's moves in as deduped,
                   legality-checked variations (ADR-0032); merge.rs merge_games folds
                   many mainlines in, frequency-orders continuations + pins per-branch
                   "N games, X%" stats (#170, ADR-0033); transpositions.rs mark_transpositions
                   (#174, ADR-0035, merge_games' final step) walks the tree mainline-first
                   tagging a node whose Zobrist was already reached earlier with "Transposes
                   to the main line after 2.c4" (appends to, never clobbers, a stats/user
                   comment; idempotent) ← unit-tested
  openings.rs      pure: ECO classification (embedded lichess dataset)     ← unit-tested
  plans.rs         pure: engine-PV → per-piece trajectories (ADR 0017)      ← unit-tested
  features.rs      pure: position feature tags (material/phase/check, #33)    ← unit-tested
  threats/         pure: hanging-piece scan → red threat arrows (#123); GET /api/threats ← unit-tested
  db/              SeaORM: config (SQLite/Postgres), entities, migrations
  ingest.rs        ingest_pgn: parse PGN → store game → replay → position_index;
                   dedups per-database by source_ref (permalink, else content
                   hash, ADR-0038); imports report game_ids + duplicates   ← shared by collectors
  collectors/      GameSource trait + Lichess / Chess.com adapters
  engine.rs        UCI engine config + parsing (Stockfish, Lc0/Maia); analyse_multi (top-N MultiPV)
  review/          Mode A (#119): engine-only full-game review — classify (pure
                   buckets + accuracy), explain (pure MoveFact "why" + the seam to
                   Mode B), service.review_game, POST /api/games/{id}/analyse   ← unit-tested
  games/export.rs  pure: mainline → MoveTree (+#119 review: [%eval]/NAGs/why) for
                   GET /api/games/{id}/export?annotated= — extended-PGN download (#120) ← unit-tested
  games/           GameService: list/get + DELETE /api/games/{id} (writable-scope
                   guard like databases; drops position_index rows first, SQLite FK
                   is RESTRICT); sharing (#211, ADR-0045): games.public (m0011) —
                   get serves a public game to anyone (overrides a private owning
                   DB for reads), set_public behind delete's permission chain, PUT
                   /api/games/{id}/public ← unit-tested
  settings/        SettingsService: per-user UI prefs as one JSON blob; persists
                   engine settings engine_multipv (1..=5)/threads (1..=64)/hash_mb
                   (1..=4096), range-validated; GET/PUT /api/settings ← unit-tested
  folders/         FolderService (#164, ADR-0030): study folder tree —
                   adjacency-list `folders` table (m0007), account-level, own ∪
                   global via scope(); create/rename/reparent (rejects cycles)/
                   delete (cascades child folders + UNFILES contained studies,
                   enforced in-app since SQLite FK cascade is inert); GET/POST
                   /api/folders, PATCH (rename/move), DELETE ← unit-tested
  studies/         StudyService: study CRUD + PGN import/export + MoveTree edits;
                   analyse.rs (#162, full review-grade classification #189, ADR-0039)
                   pure node_searches/classify_search/set_quality_nag seam for the
                   "Analyse study" pass — StudyService::analyse_study MultiPV=2
                   searches the position before *and* after every move (mirrors
                   review::service::review_game; a node's before-position is its
                   parent's after-position, so the search is cached by FEN and costs
                   one call per node, not two), engine-fills a White-perspective
                   [%eval] on every non-terminal node and classifies the move via
                   review::classify, replacing any prior move-quality NAG ($1..$6)
                   with the fresh one (comments/shapes/positional NAGs are never
                   touched); returns the refreshed study plus an AnalyseStats
                   classification/accuracy roll-up; POST /api/studies/{id}/analyse
                   also takes optional plan_lines/threats (#191, ADR-0042): sending
                   either — even 0/false — additionally runs regen_shapes.rs
                   StudyService::regenerate_shapes, a separate engine walk (own file,
                   mod.rs/routes.rs are over the file-size cap) that re-runs
                   study_gen::plan_shapes over the study's *existing* tree and merges
                   fresh shapes in per node via merge_shapes (drops only the
                   generated-brush shapes — plan1..plan3/plan1d..plan3d/threat, see
                   plan_shapes.rs's generated_brush — a node with nothing generated,
                   or every layer off, has its stale generated arrows stripped;
                   user-drawn shapes are never touched); clear_shapes.rs
                   StudyService::clear_shapes (own router in clear_shapes_route.rs)
                   bulk-removes shapes tree-wide in one call, {scope: "generated" |
                   "all"}, POST /api/studies/{id}/clear-shapes ← unit-tested;
                   folders (#164, ADR-0030): studies carry folder_id (organize) +
                   origin_game_id (analysis↔game); set_folder, studies_for_game, and
                   create_from_game (mainline → MoveTree, optional engine review via
                   the #120/#162 annotated_tree seam) back PUT /api/studies/{id}/folder,
                   POST /api/games/{id}/save-as-study, GET /api/games/{id}/studies;
                   merge_danger.rs merge_danger (ADR-0032, eval/roles update #177):
                   graft an engine-walked DangerTree into an existing study as
                   deduped variations (folds via danger_generate::to_variation_tree
                   → move_tree_from, then MoveTree::graft_subtree, which now returns
                   the newly-added (src_id, dst_id) pairs); every node the graft
                   actually creates — never one it only follows — is annotated from
                   its DangerTag: [%eval], a short role comment quoting the verdict's
                   figures, and a !/?! NAG (no LLM); own router in
                   merge_danger_route.rs (routes.rs is over the file-size cap), POST
                   /api/studies/{id}/merge-danger returns the refreshed study plus
                   added_nodes/weapons/cautions ← unit-tested;
                   merge.rs merge_games (#170, ADR-0033; opening cutoff #196): fold many
                   games' mainlines into one repertoire study via pure MoveTree::merge_games
                   (max_plies truncates each game's SAN list before folding, None/0 = every
                   ply → SAN-follow dedup → frequency-order children → pin "N games, X%
                   (labels)" stats on branch points → mark_transpositions, #174, ADR-0035;
                   standard-start only), into a new study or an existing one, threaded through
                   HTTP/MCP/frontend (FE dialog defaults to 30 plies/15 moves, 0 = whole
                   games), POST /api/studies/merge-games;
                   mark_transpositions.rs (#174, ADR-0035): standalone
                   StudyService::mark_transpositions re-runs the same pure pass on a study
                   built/edited some other way, POST /api/studies/{id}/mark-transpositions
                   (own router in mark_transpositions_route.rs, like danger_route.rs, since
                   routes.rs/mod.rs are already over the file-size cap) ← unit-tested;
                   add_line.rs add_line (#173, ADR-0032): the position-explorer "Add
                   line to study" action — builds a linear MoveTree from a flat SAN
                   list (games/export::linear_tree) and grafts it via
                   MoveTree::graft_subtree/resolve_line (dedup + an optional stats
                   comment on the line's final node), into a new study or an existing
                   one, POST /api/studies/add-line (add_line_route.rs) ← unit-tested;
                   sharing (#211, ADR-0045): studies.public (m0011) — read_scope
                   widens get/studies_for_game with the public arm (the anonymous
                   caller's ONLY arm: global non-public studies stay off the
                   anonymous tier), set_public via PUT /api/studies/{id}/public
                   (public_route.rs, own router — routes.rs is over the cap)
  ai/llm/          LlmProvider trait + DTOs (Epic 9 annotation seam); concrete client
                   entanglement.rs StackLlmProvider (#198 step 6): resolves per-user
                   (provider, model) via the agent engine's ModelResolver, drains the
                   backend stream to one CompletionResponse; routes get it per request
                   from AppState::llm_for(&user) (None ⇒ 503, no process-wide provider);
                   AppState::llm_for_choice (#214) honors an explicit per-request
                   provider/model pick on POST /api/studies/generate{,-danger-map}
                   (`provider` requires `model`; a bad choice is the caller's 400,
                   distinct from the unconfigured 503) ← unit-tested
  ai/providers.rs  ownership-aware ProviderService over llm_providers (#20, per-user #198):
                   own + global (admin) rows, keys write-only (has_key flag), per-owner
                   is_default, resolve_default_for (own → global → None); ProviderInfo
                   carries `models` (#214: own model first + builtin-catalog donations
                   for a same-named provider, deduped — the SPA model picker's source,
                   with FE helpers in lib/providers.ts + shared ModelSelect.vue used by
                   both generate dialogs)     ← unit-tested
  ai/agent/        embedded entanglement 0.6.0 agent engine (#198, ADR-0040 — replaces
                   the hand-rolled ai/assistant; m0008 drops its tables, adds
                   agent_grants/agent_events/agent_sessions): SYSTEM_PROMPT +
                   GATED_TOOLS approval gate (ADR-0025); provider_store.rs
                   AgentProviderStore — cached per-user UserProviderStore over
                   llm_providers (user rows over globals, house fallback = global rows
                   else ANTHROPIC_API_KEY; DEFAULT_PIN `~default` sentinel: the build
                   profile pins it, the resolver maps it to the caller's default row
                   at session start); policy.rs AgentPolicy (GATED_TOOLS→Ask,
                   Session grants in-memory, Always grants persisted in agent_grants);
                   tools.rs BridgedTool (MCP registry 1:1, session→CurrentUser scoping,
                   32KiB output cap); persistence.rs DbRecordSink (bounded channel →
                   agent_events, ord per root); sessions.rs SessionService (list/create/
                   open/delete, ownership fail-closed, integrity_gap-guarded resume);
                   engine.rs AgentEngine::start (Holly + tool executor + persistence tap
                   + throttle responder + index subscriber; idle_ttl 30min; compaction
                   on the session's own model)  ← unit-tested
  study_gen/       study-gen stages (Epic 9): tree (#29) builds a pruned VariationTree
                   (TreeConfig.max_children_by_depth tapers branching with depth —
                   broad near the root, narrow on deep main lines, #160;
                   TreeConfig::default() is max_depth 16/taper [4,3,3,2,2,1]/max_nodes
                   200, #196, ADR-0033 update — applies when a caller omits `tree`);
                   features.rs (#30) pure pawn-structure & key-square concepts;
                   annotate.rs (#31) batch LLM annotation pass + verification loop
                   (tool-free prompt, claims checked vs engine/DB before commit);
                   generate.rs (#115) orchestrator: tree → (optional plan/threat
                   shapes) → annotate/verify → persist a study; exposed via POST
                   /api/studies/generate (NOT MCP, ADR-0027);
                   plan_shapes.rs (ADR-0028→0029, clear semantics #191/ADR-0042)
                   pure pass: pin engine "plan" PV trajectories (plan1..plan3) +
                   static "threat" arrows onto every node as shapes; opt-in via
                   generate `plan_lines`/`threats` and the MCP `opening_tree` tool;
                   generated_brush recognizes plan1..plan3/plan1d..plan3d (the
                   frontend live-overlay's dimmed brushes)/threat, and merge_shapes
                   replaces only those in a node's existing shapes — user-drawn
                   ones always survive — the seam `studies::regen_shapes` reuses to
                   regenerate shapes on an *existing* study;
                   danger.rs (#131, ADR-0026) pure "danger-map" classifier — trap
                   weapon/hope-chess + only-move gap (engine as adjudicator);
                   danger_tree.rs (#177) pure DangerNode/DangerTree/DangerTag/
                   DangerKind/DangerRole arena types (split out of spine.rs to stay
                   under the file-size cap, mirrors VariationTree in tree.rs);
                   DangerTag.eval carries the tagged node's own White-perspective
                   [%eval] (tree::white_eval, shared with studies::analyse's #162
                   seam), flipped from the same PV1 score that already drove the
                   trap/only-move verdict — no extra engine call — None on an
                   Off-book node (never searched);
                   spine.rs (#139) PGN-repertoire walk: per opponent position runs
                   analyse_multi (movetime/variation) → reachability/trap/only-move
                   /attack → a tagged DangerTree (Weapon/Caution/Off-book — user-facing
                   text says "Not in your repertoire", #194); the trap test's
                   "tempting reply" (PV2) is weighted by its DB frequency among
                   human replies and the mate-only/single-line case is explicit
                   (#176, ADR-0026 update; thresholds still unmeasured — FE overlay
                   labelled "experimental"). Reachability/miss-rate/bait-frequency
                   compare DB SAN to the spine's own spelling via
                   pgn_tree::san_core (a spine `Bb5` matches a DB `Bb5+`) and walk
                   `answered ∪ kept` so a prepared reply absent from/below the cut
                   in the DB stats is still expanded, never silently dropped
                   (#194, ADR-0026 update);
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
                   works on a no-key install; both it and the MCP `danger_map`
                   tool take an optional `database_id` to scope reachability
                   stats to one database instead of pooling every one visible
                   to the caller (#194, ADR-0026 update);
                   seed.rs (#155) LLM-free seed seam: convert a built tree to a
                   MoveTree (move_tree_from, carries start_fen) → create_with_tree;
                   backs the data tools' `save_as` (no LLM, no PGN round-trip)  ← unit-tested
  auth/            server-mode auth: users/sessions, Argon2, AuthService (ADR 0015)
  service_tokens/  ServiceTokenService (#193, ADR-0044): admin-only mint/list/
                   revoke over service_tokens — the only way to create a
                   scoped ("full" | "read_only" | "global_read") token besides
                   the auto-seeded local one; POST/GET/DELETE
                   /api/admin/service-tokens (routes/service_tokens.rs) and
                   `chess-base service-token create|list|revoke` CLI both call
                   it ← unit-tested
  server/          Axum app: routes, state, embedded SPA, browser launch,
                   MCP /mcp + its auth (OAuth 2.1 / service token, ADR 0016;
                   anonymous public tier #192/ADR-0043 — a server-mode request
                   with no credential resolves to CurrentUser::anonymous
                   (identity.rs) instead of 401: data reads on global databases
                   only, via a dispatch-level allowlist in routes/mcp/mod.rs
                   — list_databases/db_list_games/db_read_game/
                   db_position_report/db_reference_games/db_export_games/
                   search_headers/echo; no engine, no studies, no writes;
                   local mode and an invalid/expired credential are unchanged
                   — both still 401; consent screen + CSRF, refresh-token
                   reuse detection, scoped service tokens #193/ADR-0044:
                   CurrentUser gains read_only/global_only axes (identity.rs,
                   compose with `public`) — a read_only caller hard-denies on
                   assert_admin/assert_can_write and on any MCP tool
                   ai::agent::requires_approval flags, global_only drops
                   scope()'s own-rows arm; GET /oauth/authorize no longer
                   auto-consents — a first-time (user, client) pair is parked
                   in oauth_consent_requests and redirected to
                   GET /oauth/consent (routes/oauth_consent.rs, hand-rolled
                   HTML, client_name HTML-escaped since it's attacker-
                   controlled from POST /oauth/register), whose csrf_token is
                   the CSRF defense; approving records oauth_consents so a
                   later authorize for the same pair skips the screen;
                   public sharing #211/ADR-0045: PublicUser extractor
                   (identity.rs) + AppState::resolve_public_user — six HTTP read
                   routes (game get/tree/export/linked-studies, study
                   get/export) serve public-flagged objects to a credential-less
                   server-mode request as CurrentUser::anonymous (invalid token
                   still 401s; annotated export denied anonymously);
                   POST /oauth/register is now auth-gated; refresh_token grant
                   rotation revokes the old row in place instead of deleting
                   it (oauth_tokens.family_id/revoked) — replaying an already-
                   rotated-away refresh token revokes the whole family
                   (reuse detection), and a family has a hard 30-day absolute
                   lifetime regardless of rotation count; shared OAuth helpers
                   split into routes/oauth_shared.rs to keep oauth.rs under
                   the file-size cap).
                   routes/mcp/ tools (40, #125 then #183/ADR-0036 — symmetrical to
                   the HTTP API, one carve-out list in symmetry.rs): engine_analyse +
                   analyse_position/analyse_game; study_tools.rs study_list/create/
                   get/import_pgn/add_move/annotate/export; study_node_tools.rs
                   set_folder/set_shapes/promote_node/reorder_node;
                   study_repertoire_tools.rs merge_games/merge_danger/analyse (takes
                   optional plan_lines/threats too, #191/ADR-0042)/clear_shapes
                   ({scope: "generated" | "all"}, #191, gated);
                   db_tools.rs list_databases/db_list_games/db_read_game (+
                   `annotated` flag, #120)/db_position_report/db_reference_games;
                   db_export_tools.rs db_export_games (bulk PGN, #171);
                   game_tools.rs save_as_study/studies/tree/delete; folder_tools.rs
                   list/create/update/delete (#164); search_tools.rs search_headers/
                   position_threats; import_tools.rs import_pgn/import_sync;
                   preprocess.rs data tools opening_tree/danger_map/position_concepts
                   (ADR-0027, no internal LLM); opening_tree/danger_map take an
                   optional `save_as` to seed a study server-side (#155, study_gen::seed,
                   returns {study_id,node_count}, no tree JSON) — all thin callers of
                   the shared services. Mutating tools are gated behind the agent's
                   approval flow (`ai::agent::GATED_TOOLS`, ADR-0025/0040).
                   assistant_ws.rs (+protocol.rs): GET /api/assistant/ws — the
                   streaming assistant relay onto the agent engine (envelope over
                   InMsg/OutEvent + session CRUD, ownership-filtered both ways,
                   history replay, ping/pong); routes/providers.rs: per-user LLM
                   provider CRUD at /api/assistant/providers (keys write-only)
  bin/chess-base.rs  CLI entry (clap)
frontend/          Vue 3 + TypeScript + Vite + Pinia + Tailwind v4 + chessground
                   (strict `vue-tsc`; shared API/domain types in src/types.ts; ADR 0021).
                   Semantic design tokens + class-based dark mode in src/style.css
                   (ADR 0031): bg-surface/text-fg/border-border auto-flip under
                   `.dark`; accents good/warn/bad (green/orange/red) carry move
                   quality (lib/moveTree nagClass). MoveTree renders variations as
                   depth-indented blocks (MoveTreeLine) with per-node promote/demote
                   /delete actions; a node's stored [%eval] (issue #189) renders next
                   to its NAG glyph via lib/dangerShapes' formatEval. Engine options
                   (MultiPV/Threads/Hash) persist
                   per user via settings (lib/useEnginePrefs); analysis on by default.
                   StudyAnalysis.vue: "Remove generated arrows"/"Remove all arrows"
                   (#191, ADR-0042) call studyEditor's clearShapes action
                   (POST .../clear-shapes); "all" confirms first since it also wipes
                   hand-drawn shapes, "generated" doesn't since it never does.
                   Assistant (#198): stores/assistant.ts reconnecting WS client over
                   the pure lib/assistantStream.ts fold → AssistantView streaming
                   bubbles/tool chips/approval+question cards; ProvidersSettings
                   manages the caller's LLM providers in Settings.
                   Sharing (#213, ADR-0045): router authRedirect lets a
                   /games/:id or /studies/:id navigation through logged-out (an
                   id param on either route only — the bare list routes still
                   gate); GamesView/StudyView key onMounted and their template
                   on auth.isAnonymous (an alias of needsAuth) to skip the
                   authenticated list/browse calls and render read-only — board
                   movable/editable-shapes/persist-shapes off, MoveTree
                   non-editable, engine/LLM panels (EnginePanel,
                   GameReviewPanel, StudyAnalysis, DangerMapPanel), the folder
                   sidebar and the generate dialogs all hidden; PGN export
                   stays (PublicUser-safe on the backend). ShareToggle.vue
                   (game/study headers) is the write side: a checkbox bound to
                   the object's `public` flag (api.games/studies.setPublic,
                   PUT .../{id}/public) plus a copy-link button for its own
                   deep-link URL.
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
- `make deploy` / `make deploy-restart` — apply `../deploy.yml` (k8s `services`
  ns) / re-roll pods; the image is pinned by tag in the manifest (ADR-0037).

Always `nvm use` (Node 22, see `frontend/.nvmrc`) before raw npm commands;
set `CARGO_BUILD_JOBS=4` for cargo (the Makefile does both).

CI: `.github/workflows/ci.yml` (test/lint), `release.yml` (desktop binaries on
`v*` tags), `docker.yml` (GPLv3 image with bundled Stockfish →
`ghcr.io/xmiksay/chess-base` on `v*` tags, ADR-0037).

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

**Folders** (`folders` table, #164/ADR-0030) are an account-level adjacency-list
tree (`owner_id` NULL ⇒ global, `parent_id` NULL ⇒ root) organizing **studies**,
independent of game databases. A study carries `folder_id` (which folder; NULL =
unfiled) and `origin_game_id` (the game an analysis was built from; NULL =
standalone). Folder cascade-delete + sibling-uniqueness are enforced in
`FolderService` (SQLite FKs are off and can't be `ALTER`-added).

## Roadmap (epics → GitHub milestones)

0 scaffold (this) · 1 core domain & DB · 2 collection (Lichess/Chess.com/master) ·
3 search (header + position) · 4 studies UI · 5 engine analysis (auto-download
Stockfish/Lc0/Maia) · 6 auth/settings + roles · **7 MCP / AI-assisted studies**
(JSON-RPC `/mcp` endpoint mirroring the `site` project; `StudyService` tools) ·
8 packaging & deployment (local release binaries; server Docker + Postgres) ·
**9 LLM study generation pipeline** — the AI-studies design (ADR-0009): LLM as
annotator, engine/DB as ground truth, preprocessing stages + verification loop.
