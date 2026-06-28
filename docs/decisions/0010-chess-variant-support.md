# 0010 — Per-game variant + start position (Chess960-ready data model)

**Context.** The scaffold hardcodes standard chess: `position::position_from_fen`
parses with `CastlingMode::Standard` and `STARTPOS_FEN` is the fixed standard
array. The most-requested non-standard variant is **Chess960 (Fischer Random)** —
Lichess and Chess.com both export 960 games, so any real collection will contain
them. `shakmaty`'s `Chess` type already represents 960 positions; the only
differences are the castling mode (king-takes-rook vs. e1g1 notation, X-FEN /
Shredder castling rights) and the non-standard starting array. We want the data
model to carry this from the start rather than retrofitting columns later, even if
the runtime/UI work is deferred.

**Decision.** Treat the **variant + start position as per-game data**, not a build
constant.

- The `games` schema carries `variant` (text, default `standard`) and `start_fen`
  (nullable; `NULL` ⇒ the standard startpos for that variant). This also covers
  ordinary set-up/position games, not only 960.
- `position.rs` threads a `CastlingMode` (Standard for standard chess, Chess960 for
  FRC) instead of hardcoding `Standard`. The same `Chess` position type serves both.
- **Position search is variant-agnostic.** The 64-bit Polyglot Zobrist key
  (ADR 0003) is computed from the actual board + rights, so a position is found by
  the same lookup regardless of which variant or start array produced it — no schema
  or index change is needed for search to work across variants.
- **Scope is Chess960 only.** Other shakmaty variants (Atomic, Horde, Antichess,
  Three-check, …) are explicitly **out of scope**: they fork the legal-move,
  engine, and search semantics for marginal value here. Revisit per-variant if a
  concrete need appears.
- **Implementation is deferred.** This ADR fixes the design and the schema shape now;
  runtime parsing stays standard-only until the variant work is scheduled (tracked
  on GitHub, foundationally in Epic 1).

**Consequences.**

- `games` gains `variant` + `start_fen`; entities and the migration (Epic 1) account
  for them up front, so no later schema churn.
- PGN ingest (`collectors/`, `pgn_tree.rs`) must honor `[Variant "Chess960"]` and
  `[SetUp "1"]` / `[FEN …]` headers, replaying from `start_fen` with the right
  `CastlingMode`; games without these default to standard.
- Game replay must start from `start_fen` (when present) rather than assuming
  `STARTPOS_FEN`.
- The frontend renders from the stored start array — chessground already supports
  arbitrary initial positions — plus minimal UI to display the variant.
- Until implemented, importing a 960 game is rejected/flagged rather than silently
  mis-parsed as standard, avoiding corrupt castling data in the index.

**Status (issue #39).** The data-model and ingest/replay paths have landed:
`games.variant` + `games.start_fen` (migration `m0002`), `position.rs` threading
`CastlingMode`, and `ingest.rs` honoring `[Variant "Chess960"]` / `[SetUp]` /
`[FEN]` — replaying from `start_fen` under the right mode and indexing the
variant-agnostic Zobrist key. The guard holds naturally: a 960 array imported
without a `[Variant]` tag fails to parse under standard mode rather than being
mis-indexed. Frontend rendering of the stored start array + a variant label is
tracked separately in #8.
