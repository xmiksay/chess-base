# 0027 — MCP tools are engine/DB data primitives; the LLM lives on the client side

**Context.** Two MCP tools — `generate_study` (ADR-0009, #115) and
`generate_danger_map` (ADR-0026, #141) — ran a full LLM annotation loop
(`annotate.rs`'s batch prompt + verification pass) *inside* the tool. A tool call
therefore spun up a language model of its own.

That collides with what MCP is for. MCP is the boundary between a model (the
client) and the grounded capabilities it drives (the tools). The model that calls
`tools/call` is *the* LLM loop; a tool that starts a second one nests a model
inside a model. It also broke down concretely:

- The **embedded study assistant** (ADR-0025) is itself an LLM loop, and it drives
  the *same* in-process `ToolRegistry` the `/mcp` transport serves. With
  `generate_study` registered, the assistant's loop could call a tool that runs
  another loop — a nested, doubly-billed, hard-to-steer model call where the outer
  model cannot see or correct the inner one's annotations.
- A connected external client (e.g. Claude over `/mcp`) is already a capable
  annotator. Handing it a one-shot "generate the whole study" tool wastes that:
  it cannot ground, adjust, or interleave the annotation with its own reasoning —
  the verification loop is buried server-side.
- It forced an LLM provider to be configured server-side for a capability the
  client already supplies, so engine-only / no-key deployments lost the
  preprocessing stages (the tree, the danger walk) entirely, even though those
  stages need **no** model.

The deterministic stages those generators are built on — the pruned
`VariationTree` (`study_gen::tree`, #29), the engine-adjudicated `DangerTree`
(`study_gen::spine`, #139), the pawn-structure `Concepts` (`study_gen::features`,
#30) — are pure engine/DB ground truth. They were reachable only *through* the
LLM orchestrators, never on their own.

**Decision.** Draw the boundary at the MCP transport: **an MCP tool returns
ground-truth data and never calls a language model. The LLM that turns data into
prose is the MCP client — the counterpart on the other side of the boundary — not
a tool.**

Concretely:

- **Expose the three preprocessing stages as data tools**
  (`server/routes/mcp/preprocess.rs`):
  - `opening_tree` → `build_variation_tree` → the pruned, eval/stats/ECO/concept
    tagged `VariationTree` (engine required for the per-node evals).
  - `danger_map` → `walk_danger_spine_live` → the tagged `DangerTree` plus a flat
    roles digest (engine required — the walk *is* `analyse_multi`).
  - `position_concepts` → `concepts_of_fen_with` → the pure `Concepts` (no engine,
    no DB).
  They return structured JSON only; the client annotates and persists via the
  existing `study_*` tools.

- **Remove `generate_study` and `generate_danger_map` from the MCP registry**, and
  from the embedded assistant's tool surface (it drives the same registry, so the
  removal also kills the nested-loop path there). The assistant's system prompt now
  steers it to scaffold with `opening_tree` / `danger_map` / `position_concepts`,
  then write the annotations itself.

- **Keep the orchestrators and their HTTP routes.** `generate_study_live` /
  `generate_danger_study_live` and `POST /api/studies/generate{,-danger-map}`
  stay: the SPA's one-shot "generate a study for me" button is an *application*
  feature, not an MCP tool, and it is fine for the app's own backend to call its
  own provider. ADR-0027 is about the **MCP surface**, not about banning
  server-side LLM use everywhere.

**Layering.** No new logic — the tools are thin callers over the existing
`study_gen` live wrappers and pure functions, mirroring the `db_tools` /
`analysis` pattern. Engine presence is checked up front and a miss returns a
graceful `isError`. The preprocessing tools are read-only, so the assistant runs
them without approval (only the `study_*` mutations stay gated).

**Consequences.** The MCP surface becomes a clean set of grounded primitives an
agent composes, with the model always on the outside where it can be observed and
steered; the nested-LLM-loop in the assistant is gone. `position_concepts` works
on engine-less / no-key deployments, and `opening_tree` / `danger_map` need only
an engine. The trade-off: a client that wants a finished study now does the
compose-and-annotate work itself (tree/danger → annotate → `study_create` +
`study_add_move`/`study_annotate`) instead of one `generate_study` call — which is
the point, since that client is a language model. The app keeps the one-shot path
over HTTP. Supersedes the MCP-transport half of ADR-0026 (#141).

## Addendum (#155) — `save_as`: seed a study without the round-trip

Returning the whole tree so the client can hand-serialize it into PGN and re-import
it is the slow path the data tools left open: a 120-node `VariationTree` is ≈104k
chars (overflows the tool-result budget), the hand-written PGN got variation
placement wrong (`illegal san`), and the server ends up building a tree, shipping
it out, and rebuilding an equivalent one. The server already builds the tree and
already knows how to persist one.

So `opening_tree` / `danger_map` take an optional `save_as { database_id, name,
global? }`:

- **absent** → return the tree as data (unchanged behaviour);
- **present** → build the tree, convert it to a `MoveTree`
  (`study_gen::annotate::move_tree_from`, carrying the set-up `start_fen` per
  ADR-0028), persist it via `StudyService::create_with_tree`, and return only
  `{ study_id, node_count }` — no tree JSON.

This stays within the ADR-0027 boundary: **no language model runs anywhere in the
path.** Seeding is exactly what `generate_study` does *minus* the LLM annotation
step; the model that layers the prose is still the MCP client, via `study_annotate`.
The seam lives in `study_gen::seed` (`seed_study_from_tree` /
`seed_study_from_danger`), unit-tested with fake evaluator / analyzer / continuation
source against an in-memory `StudyService`; the MCP tools stay thin callers. Every
move is `apply_san`-validated during the build, so the seeded tree is correct by
construction — the hand-PGN structural bugs cannot occur.
