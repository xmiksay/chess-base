# 0032 — Merge a danger map into an existing study

**Context.** The engine-only danger-map walk (`POST /api/studies/danger-map`,
#156 / ADR-0026) returns a raw `DangerTree` of tagged opponent replies so the SPA
danger overlay works on a no-key install. Until now the only way to *keep* that
walk was to generate a whole new study (the LLM `generate-danger-map`, #141, or
the `save_as` seed, #155). There was no way to fold the dangerous lines into a
study the user is already editing — e.g. graft the traps for a line onto their
existing repertoire chapter, without an LLM and without duplicating moves they
already have.

**Decision.** A pure tree graft in the `pgn_tree` layer plus a thin
`StudyService` method and route that reuse the existing danger→tree converters.

- **`MoveTree::graft_subtree(at, src) -> usize` (pure, `pgn_tree`).** Grafts one
  `MoveTree`'s moves into another under node `at` (defaults to the root), returning
  the count of newly added nodes. It walks `src` from its root and, for each move,
  **follows** an existing child with the same SAN (compared without the trailing
  `+`/`#` marker) when present — so a re-graft is idempotent — else **appends** a
  new child (a variation). Each move is validated for legality against the running
  position (`position::apply_san`, standard castling mode); an illegal or
  unparseable move and its whole subtree are skipped, never panicking. An unknown
  `at` or a corrupt line to it grafts nothing. Staying in `pgn_tree` keeps the
  graft I/O-free and unit-tested; grafting `MoveTree`→`MoveTree` (rather than
  taking a `VariationTree` directly) avoids a module cycle, since `VariationTree`
  already depends on `pgn_tree`.

- **`StudyService::merge_danger(user, study_id, danger, at_node_id?)`.** Loads the
  writable study + its `MoveTree` (the same guard `promote`/`delete` use), folds
  the `DangerTree` into a source `MoveTree` via the **existing** converters
  (`study_gen::danger_generate::to_variation_tree` → `annotate::move_tree_from` —
  the same fold the danger-map generator and the `save_as` seed use), grafts it at
  `at_node_id` (default root), persists once, and returns the refreshed
  `studies::Model`. **Move-only**: the engine walk carries no prose, so roles /
  eval / comments are not grafted — just the moves as variations.

- **`DangerNode.san` / `tag` gain `#[serde(default)]`.** The walk serializes a
  `DangerTree` to the client (root omits `san`/`tag` via `skip_serializing_if`),
  and the overlay POSTs it straight back, so those fields must round-trip back
  through deserialization.

- **HTTP.** `POST /api/studies/{id}/merge-danger` with body
  `{ tree: DangerTree, at_node_id?: number }` returns the refreshed `StudyView`
  (full move tree) so the editor re-renders the grafted variations from one
  response. A thin caller of `merge_danger`; ownership/write gating and error
  mapping reuse the existing `StudyError` surface.

**Consequences.** A user can graft an engine-walked danger map onto any study they
own (no LLM, no PGN round-trip, no duplicate moves on re-merge), with the graft
logic reused from the pure `pgn_tree` layer and the danger→tree fold reused from
`study_gen`. Because the graft is move-only and deduped, it composes with later
manual annotation: the variations land where the user is, and they add the prose.
Carrying the danger roles/eval into the graft (as comments or `[%eval]`) is a
possible future extension but out of scope here.
