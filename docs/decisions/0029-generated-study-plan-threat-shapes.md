# 0029 — Plan & threat arrows baked into generated studies

**Context.** Study generation (`POST /api/studies/generate`, `study_gen::generate_study`)
produced a move tree of verified LLM prose + NAGs but no board arrows. The engine
"Plans" overlay (per-piece PV trajectories, ADR-0017) and the static "Threats"
scan (hanging pieces, ADR-0024/#123) existed only as *live, non-persisted*
overlays in the analysis board — there was no way to ask the generator to pin them
onto study nodes so they ship with the study and export to PGN. Node-level pinned
shapes already exist (`pgn_tree::Node.shapes`, #61) and round-trip through PGN as
`[%cal]`/`[%csl]`; only the generation-time *population* was missing. Separately,
the MCP `opening_tree` data tool (ADR-0027) returned the bare `VariationTree` with
no arrows, so an MCP client had to recompute plans itself.

**Decision.**

- **A `VariationNode` carries `shapes`, populated by one pure-ish pass.** A new
  `study_gen/plan_shapes.rs` walks the built `VariationTree` and, per node FEN,
  emits the top-`N` plan trajectories (each line `i` under brush `plan{i}`,
  capped at `MAX_PLAN_LINES = 3` to match the frontend's registered
  `plan1..plan3` brushes) plus the threat arrows when requested. Plan PVs come
  through the existing `MultiAnalyzer` seam (`spine.rs`); threats are the pure
  `threats::threats` scan, so threats-only needs no engine. `move_tree_from`
  copies `node.shapes` into the committed `MoveTree`, and `opening_tree`
  serializes the same field — one population path, two consumers.

- **Annotate every node, opt-in, off by default.** `GenerateParams` gains
  `plan_lines: u8` + `threats: bool`; both default off so existing behavior and
  rows are unchanged. Coverage is every node (the natural "what's the plan here"
  per position); the cost is one multi-PV search per node, paid only when plans
  are requested.

- **Shapes are data, never prompt.** The pass runs *after* `build_tree` and
  *before* `annotate_tree`, but `build_prompt` only reads `san`/`eco`/`concepts` —
  the arrows never enter the LLM context, preserving ADR-0009 (engine/DB are
  ground truth; the model annotates blind).

- **Exposed on HTTP and MCP, not as a new MCP tool.** Generation stays HTTP-only
  (ADR-0027 keeps no LLM inside MCP tools), so the MCP home for the parameters is
  the existing `opening_tree` data tool: it gains optional `plan_lines`/`threats`
  args and returns the arrows as node `shapes` — still pure engine/DB data, no
  model. A dedicated depth-based `EnginePlanAnalyzer` sources the PVs under the
  generator's own depth budget (vs. `EngineMultiAnalyzer`'s movetime + floor-2,
  which exists for the danger-map gap).

**Consequences.** Plans/threats pin onto a generated study in one pass and export
to PGN unchanged (the `shapes` codec already round-trips). The study board renders
them with no frontend change — `Board.vue` already registers `planBrushes()` +
`overlayBrushes()` (plan1–3 / threat) on every board. A 4th+ plan line is rejected
rather than drawn brush-less. Computing plans for every node is the cost when
enabled; it is opt-in, so the default generate path is unaffected.
