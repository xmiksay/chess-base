# 0042 — Generated plan/threat arrows get clear semantics + a bulk clear

**Context.** Issue #191 (part of #200, sibling of #190/ADR-0041). A study
generation pass (`POST /api/studies/generate`, MCP `opening_tree`/`danger_map`
with `save_as`) can pin engine "plan" PV arrows and static "threat" arrows onto
every node (`study_gen::plan_shapes`, ADR-0028/0029). Once persisted, those
arrows had no bulk removal path — only the per-node `PUT
.../nodes/{id}/shapes` (MCP `study_set_shapes`), one call per node. ADR-0039
explicitly called this out as a known gap: *"`plan_lines`/`threats`
shape-pinning ... was considered for `analyse_study` too but is out of scope
here — no backend seam exists yet to pin PV/threat arrows onto an *existing*
tree's nodes."*

**Decision.**

- **`generated_brush`/`merge_shapes` (`study_gen::plan_shapes`) are the one
  place "is this arrow ours?" is decided.** A brush is generated when it's
  `plan1..plan3` (full-opacity plan arrows), `plan1d..plan3d` (the frontend
  live-overlay's dimmed counterparts, `lib/plansToShapes.ts` — previously
  unrecognized, a latent hole since a pinned dimmed line could never be
  replaced), or `threat`. `merge_shapes(existing, generated)` drops every
  existing shape whose brush is generated and appends the fresh ones —
  anything the user drew by hand (any other brush) always survives.
- **A shapes pass never skips a node — "off" now means "strip," not
  "no-op."** `study_gen::plan_shapes::apply_shapes` used to bail out entirely
  when `cfg.is_off()` and overwrote a node's shapes outright; it now always
  walks every node and merges via `merge_shapes`. Both changes only matter when
  the pass runs over a tree that may already carry shapes (the new
  `regenerate_shapes` pass below) — a freshly built generation tree has no
  shapes yet either way. A node whose PV can't be traced, or that's terminal,
  naturally gets an empty `generated` list and so has its stale arrows
  stripped along with everything else, with no separate code path.
- **The gap ADR-0039 flagged is closed: `StudyService::regenerate_shapes`**
  (`studies/regen_shapes.rs`, kept out of `mod.rs`/`routes.rs` — both already
  over the file-size cap) walks an *existing* study's tree (root position plus
  every move-bearing node's post-move FEN) and merges fresh plan/threat shapes
  in, exactly like a generation pass but over already-persisted nodes. It's a
  separate engine walk from `analyse_study`'s classification MultiPV=2 search,
  not fused into it: that search runs at a node's *before* position for
  best/second-best move ranking, the wrong PV source for a node's own plan
  arrows (which trace *from* the node's position) — reusing it would need
  bumping MultiPV past 2 for every node just to source plan lines nobody asked
  for on that call.
- **`POST /api/studies/{id}/analyse` (MCP `study_analyse`) gained optional
  `plan_lines`/`threats`.** Sending either field — even
  `{plan_lines: 0, threats: false}` — opts into `regenerate_shapes` in the same
  call, right after classification; omitting both leaves shapes untouched
  (unchanged from pre-#191 behavior, so existing callers are unaffected). This
  is deliberately the same endpoint/tool #189 already ships, not a new one:
  the checklist that flagged this gap treats it as one "analyse" pass.
- **Bulk clear: `POST /api/studies/{id}/clear-shapes` (MCP
  `study_clear_shapes`, gated in `GATED_TOOLS`),** `{scope: "generated" |
  "all"}` — `StudyService::clear_shapes` (`studies/clear_shapes.rs`).
  `"generated"` is `merge_shapes(existing, [])` per node (keeps hand-drawn
  shapes); `"all"` clears every shape regardless of origin. Listed in
  `symmetry.rs`'s manifest like every other mutating route.
- **Frontend:** `StudyAnalysis.vue` gained "Remove generated arrows" (no
  confirmation — it never touches hand-drawn shapes) and "Remove all arrows"
  (asks first, since it also wipes anything the user drew) next to the
  existing "Analyse study" button, backed by `studyEditor.ts`'s `clearShapes`
  action.

**Consequences.** `regenerate_shapes` costs one engine call per node when plan
lines are requested (mirrors `study_gen::generate`'s own pass over a fresh
tree) — there is no FEN-cache reuse with the classification pass, so analysing
with shapes on is slower than a plain classify. Re-analysing a study with
shapes off after a manual `set_shapes` edit is safe: only the generated
brushes are ever touched. This is the annotation (persisted) layer's clear
control; the live-overlay "Clear arrows" (ADR-0041) is a separate, unrelated
button that never touches persisted node shapes at all.
