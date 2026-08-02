# 0033 — Merge multiple games into one repertoire study

**Context.** Building a repertoire from a player's games (e.g. every Carlsen
Najdorf) meant N manual "Save as analysis" clicks producing N isolated
single-mainline studies (#164 `create_from_game`). The merge primitive already
existed — `MoveTree::graft_subtree` (#169 / ADR-0032) grafts a tree in as deduped,
legality-checked variations, and `games/export.rs::linear_tree` turns a mainline
into a `MoveTree` — but nothing folded *many* games into one tree, ordered the
result by how often each continuation was actually played, or surfaced per-line
frequency/score so a user can tell a main weapon from a sideline.

**Decision.** A pure frequency-merge in the `pgn_tree` layer plus a thin
`StudyService` method and route, reusing the existing dedup/graft mechanics.

- **`MoveTree::merge_games(&[MergeGame]) -> usize` (pure, `pgn_tree::merge`).**
  Each `MergeGame` is caller-resolved pure data: mainline SANs, a display `label`
  (`"Carlsen–Nepo 2023"`), and a White-perspective `white_score` (`Some(1.0/0.5/
  0.0)` or `None`). It walks each game from the root with the same **SAN-follow
  dedup** `graft_subtree` uses (follow a child matching the SAN sans `+`/`#`, else
  append a variation; an illegal move in the running position ends that line), so a
  re-merge of the same games is idempotent. Along the way it accumulates per-node
  stats (game count, score sum, sample labels), then:
  - **orders every node's children by frequency** (stable sort, most-played first)
    so the most common continuation becomes/stays the mainline; and
  - **pins a stats comment on each branch alternative** (a node whose parent has
    ≥2 children): `"12 games, 71% (Carlsen–Nepo 2023, …)"`. The percentage is the
    **mover's** expected score (White's on odd plies, its complement on even), from
    the games with a known result; it is omitted when none do. A user comment is
    left untouched; a prior stats comment is refreshed (keeping re-merge idempotent).

- **Standard start only.** The merge always folds from the standard initial
  position, never a set-up `[FEN]` — different move orders into the same tabiya must
  stay visible as separate branches (transpositional entry orders are preparation
  value). `StudyService` skips source games with a set-up start or non-standard
  variant, and rejects grafting into a study that itself has a set-up start.

- **`StudyService::merge_games(user, game_ids, study_id?, name?, folder_id?)`**
  (in `studies/merge.rs`, kept out of the over-cap `mod.rs`). Resolves each game via
  `GameService::get` (enforcing own ∪ global visibility), builds the `MergeGame`
  list, then either grafts into an existing writable study (`study_id`) or creates a
  new caller-owned study (`name` required) from the standard start, filed into
  `folder_id`. `origin_game_id` is left `None` — a repertoire has many sources.

- **HTTP.** `POST /api/studies/merge-games` with body
  `{ game_ids: [..], study_id?, name?, folder_id? }` returns the resulting
  `StudyView` — `201` when a new study is created, `200` when grafting — so the
  client re-renders the merged tree from one response. A thin caller; ownership/
  write gating and error mapping reuse the existing `StudyError` surface.

**Consequences.** A user multi-selects games in `GamesView` and folds them into one
frequency-ordered repertoire study in a single request, with the merge logic reused
from the pure `pgn_tree` layer (unit-tested without a DB) and the visibility/write
gating reused from the services. The stats comment is intentionally minimal (count +
score + sample labels); the mover-perspective score sidesteps needing to know which
colour the repertoire is for. Carrying richer per-line stats (W/D/L split, opponent
Elo) or annotating known transpositions between branches are possible follow-ups
(the issue flags a separate transposition-annotation task). The header-search-results
entry point is a separate UI issue.

## Update (issue #196) — opening cutoff + saner `TreeConfig` defaults

**Problem.** Nothing capped how far a game's mainline folded in: every source
game's line ran to its final move, so a "repertoire" study read as a pile of
whole games (lines 60+ plies deep), branching only where the games happened to
diverge — unusable as opening prep. Separately, the best-line generator
(`study_gen::tree::TreeConfig::default`) shipped with `max_depth: 6` (3 moves),
so a generated study felt trivial unless the caller manually widened it.

**Decision — merge cutoff.** `MoveTree::merge_games` (pure, `pgn_tree::merge`)
takes a new `max_plies: Option<u32>`: each game's SAN list is truncated to its
first N plies *before* folding, so the frequency ordering, branch-point stats,
and `mark_transpositions` pass all run on the already-truncated tree — the
stats a branch node carries are always about the games that actually reach it,
truncated or not. `None` or `Some(0)` folds every ply (today's behavior,
preserved for existing MCP/HTTP callers that don't pass the field). Threaded
straight through as an `Option<u32>`: `StudyService::merge_games`, the
`POST /api/studies/merge-games` body, the MCP `study_merge_games` schema, and
`frontend/src/api.ts`. `MergeGamesDialog.vue` exposes it as a "Max length
(moves)" number field — default **15 moves (30 plies)**, `0` = whole games;
converted to plies (`× 2`) before the request. `GamesView.vue`'s inline
multi-select flow doesn't expose the field and stays uncapped, matching its
existing prompt-only UI.

**Decision — `TreeConfig::default()`.** Raised from `max_depth: 6, max_children:
3, max_nodes: 64` to `max_depth: 16` (8 moves) with a `max_children_by_depth`
taper of `[4, 3, 3, 2, 2, 1]` (broad near the root, narrowing to the principal
line deep, issue #160) and `max_nodes: 200` to keep the per-position engine-eval
budget bounded. This only changes behavior for callers that don't fully specify
`tree` — omitting it entirely (the MCP `opening_tree`/`danger_map` data tools,
`POST /api/studies/generate` without a `tree` body) picks up the new
`max_depth`/`max_children`/`max_nodes`/`min_frequency`/`eval_margin_cp` **and**
the taper; a caller that sends a partial `tree` object picks up the new scalar
defaults for any field it doesn't name, but **not** the taper —
`max_children_by_depth` carries its own field-level `serde(default)` (opt-in
per #160) that resolves to `None` regardless of the struct default whenever a
`tree` object is actually present in the request. `GenerateStudyDialog.vue`
already sends its own explicit `max_depth`/`max_children` and is unaffected
either way. Values are a reasoned default, not a benchmarked one; no
default-node-count assertions exist in the test suite to pin an exact number,
so this can be retuned without a breaking migration.

**Consequences.** A merged repertoire study now reads as opening prep by
default while remaining one flag away from the old whole-games behavior.
Generated studies read as more than a token opening line out of the box.
Neither change touches the pure layer's existing dedup/idempotency guarantees
(re-merging is still a no-op; re-running `mark_transpositions` on an
already-tagged tree still doesn't duplicate tags).
