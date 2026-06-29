# 0019 — Interactive analysis mode: `analyse_position` bundles engine + DB + features

**Context.** Epic 9 distinguishes two annotation paths over the same ground
truth (ADR-0008/0009): the **batch** pipeline is code-orchestrated and tool-less,
while the **interactive** mode (issue #33) is the tool-using one — a connected
client (Claude Desktop / Claude Code, or the embedded assistant #20) is agentic
over the MCP facades to "look at this position and explain it." The grounding
sources already exist as separate MCP tools: `engine_analyse` (the pooled engine
facade, #27/ADR-0014) and `db_position_report` (the pre-chewed DB layer,
#28/ADR-0018). The remaining input — **feature tags** (#30) — was not yet
implemented, and nothing tied the three together for a single "explain this
position" call.

**Decision.** Two pieces.

1. A new **pure** `features` module (`src/features.rs`): `features_of_fen(fen) ->
   Features` derives *factual* descriptors straight from the board — material
   census + signed balance (P=1/N=B=3/R=5/Q=9), game phase (by remaining
   non-pawn material weight), side to move, check/checkmate/stalemate,
   insufficient material, legal-move count, castling rights — plus a short
   human-readable `tags` list. These are grounded facts the model could verify
   itself, not opinions. It sits in the same I/O-free layer as `position` /
   `plans` and is fully unit-tested. The deeper pawn-structure / key-square
   classification (#30) is intended to *extend this same module* without changing
   callers; #33 ships the factual baseline so it is self-contained.

2. A new MCP tool `analyse_position` (`server/routes/mcp_analysis.rs`): the
   one-shot interactive entry point. It validates the FEN once (via the feature
   extractor), then bundles `{ fen, features, database, engine, notes }` —
   reusing `PositionReportService` and the pooled `EngineService` **verbatim** —
   into one grounded snapshot, serialized as JSON. The model is instructed to
   base explanations on these figures and never invent lines. The unbundled
   `engine_analyse` / `db_position_report` / `db_reference_games` tools remain
   registered for an agent that wants to drill in further.

A missing or failing engine is **not** a hard error: `engine` is `null` and a
`notes` entry explains the omission, so an explanation is still grounded on the
DB report and features. An invalid FEN fails fast (before any DB/engine work) as
an `isError` outcome; `SearchError` is mapped through the shared non-leaking
`report_error` helper (raw `DbErr` never reaches the client). Scope follows the
caller's identity (own ∪ global, ADR-0007/0011/0016) like every other tool.

**Consequences.** "Explain this position" is one tool call returning all three
grounded sources, minimizing round-trips and keeping claims tool-sourced. No new
HTTP route — the surface is MCP-only, matching the issue (`server/routes/mcp.rs`,
reusing the engine facade, DB layer, feature extractor). The engine and DB
aggregation each live in exactly one place; `analyse_position` is a thin
composer. Tests: unit tests in `features.rs` (startpos, imbalance, mate,
stalemate, endgame phase, insufficient material, invalid FEN) and end-to-end MCP
tests in `tests/mcp.rs` (bundled DB stats + features without an engine, invalid
FEN, tool listing, and an engine-gated bundle behind `CHESS_BASE_TEST_ENGINE`).
