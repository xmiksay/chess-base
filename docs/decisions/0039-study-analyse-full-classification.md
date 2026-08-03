# 0039 — "Analyse study" does full review-grade classification, not just eval

**Context.** `StudyService::analyse_study` (issue #162) filled a
White-perspective `[%eval]` on every study node but never classified a move —
no `!`/`?!`/`?`/`??` NAG, no accuracy. That made it inconsistent with the rest
of the app: full-game review (`POST /api/games/{id}/analyse`, issue #119) and
"Save as analysis" both produce eval + classification + accuracy in one pass.
Worse, the frontend never even rendered the stored evals, so clicking
"Analyse study" visibly changed nothing (issue #189).

The classifier (`review::classify::classify`) needs, for the position *before*
a move: the engine's best line, the runner-up (to spot an "only move"), and
the played move's rank. `analyse_study` only ever searched the position
*after* each move — the classifier's inputs were never computed.

**Decision.** `analyse_study` now searches **both** sides of every move,
MultiPV=2, mirroring `review::service::review_game`:

- The pure `studies::analyse` seam gained `node_searches` (FEN before *and*
  after each move, replacing the eval-only `node_fens`) and `classify_search`
  (given the before-move MultiPV lines and the after-move top score, derives
  the node's `Eval`, `Classification`, and `MoveCost` — pure, unit-tested with
  synthetic `Analysis` values, no engine needed, mirroring
  `review::service::assemble`).
- **Search reuse, not 2×.** A node's before-position is its parent's
  after-position, so `analyse_study` caches MultiPV=2 results by FEN
  (`multipv_cached`): a linear line costs one engine call per node, the same
  as the old eval-only pass, not two.
- **NAG replacement, not append.** `set_quality_nag` clears any prior
  move-quality NAG ($1..$6) on a node before writing the fresh
  `Classification::nag()` — re-analysing a study never piles up glyphs.
  Comments, shapes and *positional* NAGs (everything outside 1..=6) are never
  touched.
- Terminal after-positions (checkmate/stalemate) are classified from the move
  that reached them (`after_eval` reads the result off the board) without a
  further, impossible search.
- The response gains an `AnalyseStats` roll-up (node count +
  `review::classify::summarize`'s per-side accuracy/error-counts), returned
  over both HTTP (`POST /api/studies/{id}/analyse`) and the MCP `study_analyse`
  tool.

Frontend: `lib/moveTree.ts`'s `treeTokens` now carries a node's stored `eval`
onto its `MoveToken`, and `MoveTreeLine.vue` renders it (`formatEval`) next to
the NAG glyph — the #162 gap where evals were persisted but invisible.
`StudyAnalysis.vue` shows the returned `AnalyseStats` after a pass. The game
review graft (`lib/reviewTree.ts`) also now writes the classification NAG onto
the *played* mainline node (`setQualityNag`, the same replace-not-append rule),
not just the grafted `best_line` variation, so glyphs render in the notation
there too.

**Consequences.** "Analyse study" now costs roughly the same number of engine
calls as before (one MultiPV=2 search per node instead of one plain search),
but the pass takes noticeably longer per call since MultiPV=2 searches more
lines. A study re-analysed after a manual quality NAG edit loses that manual
NAG — expected, since the whole point is that the engine's classification is
now authoritative for that node. `plan_lines`/`threats` shape-pinning (as
`generate` supports) was considered for `analyse_study` too but was out of
scope here — no backend seam existed yet to pin PV/threat arrows onto an
*existing* tree's nodes, and the issue's gate never required it. That gap is
closed by issue #191/ADR-0042: `POST /api/studies/{id}/analyse` and
`study_analyse` now take optional `plan_lines`/`threats` and, when sent, also
run a `regenerate_shapes` pass alongside classification.
