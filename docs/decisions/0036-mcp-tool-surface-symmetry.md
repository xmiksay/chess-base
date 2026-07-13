# 0036 — The MCP tool surface is symmetrical to the HTTP API

**Context.** The MCP registry (`server/routes/mcp/tools.rs` + its sibling
`*_tools.rs`/`preprocess.rs`) grew tool-by-tool as each Epic 9 issue shipped,
while the HTTP API kept growing on its own track (folders, repertoire
merging, save-as-study, header search, …). By issue #183 the registry exposed
16 tools against ~45 HTTP routes: an agent driving `/mcp` (external Claude, or
the embedded assistant over the same in-process `ToolRegistry`, ADR-0025)
could read games and build a study move-by-move, but could not list studies,
merge games into a repertoire, save a game as a study, reorder/promote nodes,
organize folders, or search headers — all things the SPA could already do.
Nothing enforced that a new route shipped its MCP twin; the two surfaces had
simply drifted.

**Decision.** The MCP surface should be symmetrical to the HTTP API, with one
line already drawn by [ADR-0027](0027-mcp-data-tools-no-internal-llm.md): *an
MCP tool returns ground-truth data or performs a deterministic mutation; it
never calls a language model.* Concretely:

- **Every HTTP operation that isn't LLM-orchestrating or session/admin/infra
  scoped gets an MCP counterpart**, added in the same style as the existing
  tools — a thin wrapper over the same transport-agnostic service the HTTP
  route calls (`StudyService`, `GameService`, `FolderService`,
  `HeaderSearchService`, `ImportService`, …). No new business logic; the
  service is the single source of truth for both transports.
- **Mutating tools go through the existing per-tool approval gate**
  (`ai::assistant::GATED_TOOLS`, ADR-0025) — every tool added by #183 that
  writes data is listed there, reusing the same writable-scope guards the
  service already enforces.
- **The carve-outs stay exactly what ADR-0027 and prior practice already
  established** — nothing new is exempted:
  - **LLM orchestrators** — `POST /api/studies/generate`,
    `generate-danger-map` (ADR-0027: the client is the LLM; a tool must not
    nest a second loop).
  - **The assistant loop itself** — `/api/assistant/*` (it *is* the client).
  - **Session / admin / infra** — `/api/auth/*`, `/api/whoami`,
    `/api/settings` (per-user UI prefs), `/api/engines*` (engine admin),
    `/api/health`, OAuth discovery/token endpoints.
  - `GET /api/engine/analyse` (WS streaming) — `engine_analyse` already covers
    the request/response form.
- **A hand-maintained manifest (`server/routes/mcp/symmetry.rs`) is the
  drift guard.** Axum doesn't expose a walkable route list at runtime, so the
  check is a doc-driven table of `(method, path) → tool`, asserted against the
  live registry in a unit test, plus the carve-out list with its reasons. The
  convention going forward: **a PR that adds an HTTP route adds a manifest row
  in the same change** — either a tool name or a carve-out entry. This catches
  "route X claims tool Y but Y was never registered / got renamed"; it does
  not catch a route added without touching the manifest at all — that stays a
  code-review concern, same as it always was for keeping `.claude/CLAUDE.md`
  and `docs/architecture.md` in sync.

**What shipped.** Grouped by the file each landed in
(`server/routes/mcp/`):

- `study_tools.rs` (+`study_list` — the studies list was previously
  undiscoverable over MCP at all).
- `study_node_tools.rs` (new) — `study_set_folder`, `study_set_shapes`,
  `study_promote_node`, `study_reorder_node`: the remaining per-node/study
  mutations `study_tools.rs` didn't cover, split into their own file to stay
  under the project's 500-line cap.
- `study_repertoire_tools.rs` (new) — `study_merge_games`,
  `study_merge_danger`, `study_analyse`: the repertoire-building operations —
  folding many games into one study, grafting an engine-walked danger tree,
  filling `[%eval]`s from the engine.
- `game_tools.rs` (new) — `game_save_as_study`, `game_studies`, `game_tree`,
  `game_delete`: the game-document operations that compose more than one
  service (`GameService` + `StudyService`) or replicate a route's inline
  tree/PGN composition (there is no single service method for "a game's move
  tree" — `game_tree` mirrors `games/routes.rs::get_tree`'s inline
  `from_pgn_with_start` call).
- `db_tools.rs` — `db_read_game` gained an `annotated`/`depth` flag (the #120
  engine-annotated PGN, mirroring `GET /api/games/{id}/export?annotated=`).
- `db_export_tools.rs` (new) — `db_export_games`: bulk verbatim PGN export
  (#171), split out since adding it to `db_tools.rs` would have crossed the
  line cap.
- `folder_tools.rs` (new) — `folder_list`, `folder_create`, `folder_update`,
  `folder_delete`: the entire folder surface (#164/ADR-0030) was absent from
  MCP; `folder_update` fans out to `FolderService::rename`/`::reparent` the
  same way the HTTP `PATCH` handler does.
- `search_tools.rs` (new) — `search_headers`, `position_threats`: metadata
  search and the hanging-piece scan, previously HTTP-only.
- `import_tools.rs` (new) — `import_pgn`, `import_sync`: PGN upload and
  Lichess/Chess.com sync into a database.
- `symmetry.rs` (new, test-only) — the manifest + carve-out list described
  above.

**Consequences.** The registry grew from 18 to 39 tools; every one is a thin
service wrapper, so no business logic moved or duplicated. `ai::assistant`'s
`GATED_TOOLS` grew in step, so the embedded assistant still pauses for
approval on every new mutation exactly like the pre-existing ones. A few
routes this issue's audit found un-mirrored were deliberately left out of
scope (recorded in `symmetry.rs`'s `KNOWN_GAPS`: database CRUD, study
rename/delete/delete-node, add-line, mark-transpositions, and the
stored-game engine review at `POST /api/games/{id}/analyse`) — closing those
is future work, not a silent omission. The trade-off of the manifest being
hand-maintained rather than introspected is accepted: Axum has no public API
to enumerate a `Router`'s registered paths, and the manifest at least turns
"did we forget the MCP twin" into a one-line addition instead of a project-wide
grep.
