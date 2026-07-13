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

- **`MoveTree::graft_subtree(at, src) -> usize` (pure, `pgn_tree`).** *(Issue
  #177 changed the return type to `Vec<(usize, usize)>` — see the update below —
  this section is otherwise as originally decided.)* Grafts one `MoveTree`'s
  moves into another under node `at` (defaults to the root), returning
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
  `at_node_id` (default root), persists once, and returns the refreshed study.
  **Move-only**: the engine walk carries no prose, so roles / eval / comments
  are not grafted — just the moves as variations. *(Issue #177 lifted this for
  eval and the role verdict, and changed the return type to a
  `MergeDangerOutcome` — see the update below.)*

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

**Update (issue #177): eval + annotated grafts.** The move-only limitation above
is lifted for the eval and the role verdict, closing the gap where a Weapon at
+0.3 and one at −0.5 looked identical.

- **`DangerTag.eval: Option<Eval>`.** `spine.rs`'s `classify()` already computes
  the tagged node's own position score (`lines.first().score`, the same PV1 that
  drove the trap/only-move verdict) with no extra engine call; it is now flipped
  to White's perspective (`study_gen::tree::white_eval`, shared with
  `analyse_study`, #162) and stored on the tag. `None` for an Off-book node — no
  search ran on its own position, only on the parent that flagged it missing.
  `DangerNode`/`DangerTag`/`DangerKind`/`DangerRole`/`DangerTree` moved out of
  `spine.rs` into a new `study_gen::danger_tree` module (a pure sibling of
  `tree`'s `VariationTree`) to keep `spine.rs` under the file-size cap once the
  eval plumbing landed; the flat `study_gen::DangerTree` etc. re-exports are
  unchanged.
- **`MoveTree::graft_subtree`/`graft_children` return `Vec<(usize, usize)>`** —
  `(src_id, dst_id)` pairs for **newly added** nodes only, instead of a bare
  count — so a caller can map a grafted node back to the source tag that
  produced it and knows exactly which nodes it may still annotate.
- **`StudyService::merge_danger`** (moved to its own `studies/merge_danger.rs`
  file, same over-cap reason as `add_line.rs`) uses that pairing to annotate
  **only the nodes it just created**: `set_eval` from the tag's eval, a short
  role comment quoting the verdict's own figures (e.g. `"Weapon: trap, bounded
  downside on the best reply (+0.30)"`, `"Caution: only move, 42% miss rate"`),
  and a `!`/`?!` NAG for a Weapon/Caution. A node the graft only *followed*
  (already in the study) is never touched — the same non-destructive contract
  `analyse_study` (#162) uses for eval. The response
  (`POST /api/studies/{id}/merge-danger`, now its own `merge_danger_route.rs`
  for the same over-cap reason as `danger_route.rs`) gains `added_nodes` /
  `weapons` / `cautions` alongside the refreshed `StudyView`, so the FE "Extend
  this study" action reports "N new nodes, W Weapons, C Cautions" — or "no new
  lines" on an idempotent re-merge — instead of a silent success.
- **`to_variation_tree`/`annotate::move_tree_from`** (the LLM-pipeline
  converters `generate_danger_study` also uses) are unchanged — still fold
  `eval: None`/no comments/no NAGs. The annotation above is layered on
  afterward, in `merge_danger` itself, keyed off the original `DangerTree`'s
  tags via the id pairing — so the LLM danger-study-generation pipeline's
  verification semantics (#31: a claim only commits if ground truth confirms
  it) are untouched.
- **Overlay + roles digest.** `RoleView` (HTTP `danger_route.rs`) and the MCP
  `danger_map` tool's inline roles digest both gained an `eval` field (mirroring
  the tag), and `DangerTag.eval` naturally rides along in the full `DangerTree`
  every surface already returns — unifying the free overlay, the MCP tool and
  the merge as the same underlying figures: the overlay is now a preview of
  exactly what a merge would write into the study.
