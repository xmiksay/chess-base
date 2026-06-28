# 0017 — Plan trajectories: a pure module, thin WS/MCP callers

> Numbered 0017, not 0014: the issue (#58) predates parallel branches that
> claimed 0014 (engine facade) and a second 0016. The next free number is used.

**Context.** The engine WebSocket (`/api/engine/analyse`, ADR-0012) streams
MultiPV principal variations (`AnalysisInfo{ multipv, score, pv:[uci…] }`). A
**Plan** visualizes the *idea* behind a line: each piece that moves gets its
trajectory drawn across the whole line — for `Nf3 … Ng5`, the knight path
`g1→f3→g5`, not just the next move. This piece-chaining is pure geometry over a
legal move sequence; per the architecture layering rule it belongs in the pure,
unit-tested core, callable by both the WS handler and the future MCP endpoint —
not duplicated in the frontend.

**Decision.** Add `src/plans.rs`, a pure module built on `shakmaty`, reusing
`position::position_from_fen` and UCI parsing:

- `plan_from_pv(start_fen, pv_uci, max_moves, mode) -> Result<Plan, PositionError>`
  returns `Plan { trajectories: Vec<Trajectory> }`, each
  `Trajectory { piece: char, squares: Vec<String> }` listing the squares one
  piece visits (origin included): `["g1","f3","g5"]`.
- The traced side is the side to move in `start_fen`; **only** its pieces are
  traced. Opponent replies are applied to keep the board legal but never traced.
- **Chaining is by square continuity:** a traced move whose origin equals an
  existing trajectory's current square extends it; otherwise it starts a new
  path. Captures keep the chain (the destination is still one square). Castling
  traces the *king's* path (`e1→g1`), since `shakmaty`'s `Move::to()` reports the
  rook square for castles.
- `max_moves` caps the traced side's own plies (default `DEFAULT_MAX_MOVES = 4`)
  so the drawn arrows stay readable.
- **No panics.** Only an invalid `start_fen` errors; a truncated, illegal, or
  unparseable PV move simply stops the trace and returns what was traced so far.

The `piece` char is color-cased FEN (`'N'` White, `'n'` Black) so the renderer
knows the side without a second field. `Plan`/`Trajectory` are `serde`-derived
for the WS/MCP wire.

**Consequences.** The WS emission issue and the MCP endpoint are thin callers
over one tested function; the frontend only renders trajectories. The module is
covered by ordinary unit tests (knight chain, two pieces, capture mid-path,
castling, empty/short/truncated PV) with no engine or process. Multi-PV plans
are just one `plan_from_pv` call per `pv`; that fan-out lives in the caller, not
here. The square-continuity rule also guards against a different same-typed piece
of the same side spuriously extending a vacated square's path (the piece char
must match too).
