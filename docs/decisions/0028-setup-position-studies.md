# 0028 — Set-up position studies: `start_fen` on the move tree

**Context.** Studies stored only a `MoveTree` of SANs and always replayed it from
the standard start position: `pgn::from_pgn` ignored the `[FEN]`/`[SetUp]` header,
the studies service's `fen_at` and the SPA board both seeded a fresh `Chess()`,
and `studies` had no start-position column. So a study could not begin from a
custom position. Importing a PGN that opened on a set-up FEN — an opening tabiya,
an endgame, a tactics fragment — failed with an *illegal move* at the first move
(the SAN was legal only from the FEN, not from the standard start). `from_pgn_with_start`
existed but was only wired into the danger-map spine walk; the study import path
never used it, and there was nowhere to persist the origin even if it had.

This surfaced building a Catalan study from the position after `1.d4 Nf6 2.c4 e6
3.g3`: `study_import_pgn` rejected `3... d5`.

**Decision.** Make the set-up start position first-class on the move tree, end to
end, with **no DB migration**.

- **`MoveTree.start_fen: Option<String>`** — `serde(default, skip_serializing_if = "Option::is_none")`.
  `None` means the standard start, so every pre-existing `tree_json` blob loads
  unchanged and standard-start studies serialize byte-for-byte as before. The blob
  is the schema, so the column-free `studies` table needs no migration.
  `MoveTree::start_position()` is the single resolver (`start_fen` or startpos)
  every replay calls.

- **Import honours the header.** The `pgn` importer's visitor now captures the
  `[FEN]` tag (`Tags = Option<String>`); `from_pgn` seeds from it, recording a
  non-standard origin on the tree. `from_pgn_with_start` keeps its explicit-origin
  semantics and **overrides** any header (the spine walk's start comes from the
  request, not the movetext). The study-generation converter likewise carries a
  custom generation start through from the variation tree's root FEN.

- **Export is self-contained.** `to_pgn` re-emits `[SetUp "1"]`/`[FEN "…"]` for a
  set-up tree and numbers the movetext from the FEN's move number and side
  (`3... d5 4. Bg2`), so export → re-import round-trips. The movetext-only writer
  is shared with the Lichess-study export.

- **Replay seeds from the origin everywhere.** The studies service's `fen_at`
  takes the start FEN; the SPA's study-editor board seeds chess.js from
  `tree.start_fen` (falling back to the standard start on absence or a malformed
  FEN, so a bad origin never blanks the board).

**Consequences.** `study_import_pgn` accepts a `[SetUp]`/`[FEN]` PGN and the study
edits, exports and renders correctly from that origin; generated studies from a
custom start FEN persist their origin too. Studies remain a single JSON blob — no
schema change, no migration, old rows untouched. Standard-start studies are
completely unaffected (`start_fen` stays absent). Extends the game-level start
position (ADR-0010) to the study move tree (issue #135).
