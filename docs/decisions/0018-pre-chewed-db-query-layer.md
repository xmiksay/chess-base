# 0018 — Pre-chewed DB query layer (ECO, win-rate/frequency, transpositions, references)

**Context.** Epic 7/9 (ADR-0008, ADR-0009) make the LLM an *annotator* over
ground truth it never computes itself: the database tools must return
**synthesized** answers, not raw rows. The raw query side already exists —
`search::PositionSearchService` (issue #7) aggregates the Zobrist
`position_index` into an opening tree (per-continuation `count` + W/D/L) and
finds games reaching a position (ADR-0003) — and `openings` (issue #36) maps a
Zobrist to its ECO code+name. Issue #28 asks to compose these into one
pre-chewed report and expose it over MCP, **without** reimplementing the #7
aggregation.

**Decision.** Add `search::report::PositionReportService`, a thin layer wrapping
`PositionSearchService` (reused verbatim) plus the pure `openings` lookup:

- `position_report(user, fen) -> PositionReport` — `{ fen, zobrist (hex),
  eco: Option<{eco,name}>, total, moves: [MoveReport], transpositions: [Transposition] }`.
  - `moves` is `opening_tree`'s output with two **derived** figures layered on:
    `frequency = count / total` (share of games choosing the move) and
    `score = (white + draws/2) / decided` (White's performance, `decided =
    white+draws+black`; unknown-result games count toward `count` only).
  - `eco` is `opening_of_zobrist` on the position's hash, so an un-played but
    known opening still classifies (empty `moves`/`transpositions`).
  - `transpositions` are the **distinct move orders** that reach the same
    Zobrist: for each scoped game hitting the key, take the first (lowest-ply)
    arrival and replay its indexed moves up to that ply; identical SAN lines
    collapse to one `{ line, ply, games }`, sorted by game count.
- `references(user, fen, limit)` — scoped reference/typical games; a direct reuse
  of `games_with_position`.
- `position_reports(user, fens)` — the internal batch entry point.

Scope reuses #7's ownership rule (own ∪ global) via the denormalized
`position_index.database_id`; `visible_database_ids` is shared (made
`pub(crate)`) so scope is computed the same way in both layers.

The MCP surface is `server/routes/mcp_db_tools.rs`: `db_position_report` and
`db_reference_games`, thin handlers that serialize the service output to JSON and
map `SearchError` to a non-leaking `isError` outcome (raw `DbErr` never reaches
the client). No HTTP route is added — the report layer is internal + MCP only,
matching the issue's surface (`src/search`, `src/db`, `server/routes/mcp.rs`).

**Consequences.** The aggregation lives in exactly one place (#7); this layer
only adds ECO, the two derived ratios, and transposition reconstruction.
Transposition reconstruction scans every scoped occurrence of the position and
its games' indexed plies — bounded in practice by `database.index_depth` and the
scope, and intended for opening/structure positions; deep middlegame positions
with vast game sets would scan more, which a future cap can bound if needed.
`zobrist` is serialized as zero-padded hex to avoid 64-bit precision loss in JSON
consumers. Tests cover the seeded dataset at both layers: unit tests in
`report.rs` (ECO + frequency/score, transposing move orders, scoped references,
batch, unknown/invalid FEN) and end-to-end MCP tests in `tests/mcp.rs`.
