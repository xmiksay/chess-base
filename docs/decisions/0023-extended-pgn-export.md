# 0023 — Extended PGN export (one serializer, real `.pgn` downloads)

**Context.** Issue #120 asks for *extended* PGN export — PGN carrying the engine
analysis (per-move `[%eval]`, move-quality NAGs, the rule-based why-note) and the
existing study annotations (comments / NAGs / `[%csl]`/`[%cal]` shapes) — for both
a single game and a study. Before this, games had no export at all, and the study
export returned a JSON `{pgn}` field the SPA would have had to turn into a file.
The analysis facts already exist (the #119 `GameReview`/`MoveReview` structs), and
`pgn_tree::pgn` already serializes comments/NAGs/shapes. The risk is growing a
*second* PGN emitter for games and a parallel eval-encoding path.

**Decision.**

- **`[%eval]` is a first-class node annotation.** `pgn_tree::Node` gains an
  `eval: Option<Eval>` (`Cp`/`Mate`, always White's perspective per the PGN
  convention), `serde(default)`/skipped so existing `tree_json` loads with no
  migration. A small `pgn_tree::eval` codec (mirroring `shapes`) encodes/parses
  the `[%eval …]` comment command, and `pgn::{from_pgn,to_pgn}` round-trip it
  alongside shapes and comments. `[%clk …]` is preserved as comment text (it is
  neither an eval nor a shape), satisfying the "keep clocks" requirement for free.
- **One serializer for both surfaces.** Games reuse the same tree + serializer:
  the pure `games::export` builds a `MoveTree` from the mainline (`linear_tree`)
  or from the mainline **plus** the #119 review (`annotated_tree`: `[%eval]` on
  every ply, a NAG + why-note on the faults), then calls `pgn_tree::pgn::to_pgn`.
  No second emitter; the export → re-import → equal-tree round trip is unit-tested.
- **Real `.pgn` file downloads, standardised.** All export routes (game, study,
  Lichess-study) return an attachment — `application/x-chess-pgn` +
  `Content-Disposition` — via the shared `server::download::pgn_attachment`, not a
  JSON `{pgn}` body. The SPA fetches the text and triggers the download through
  the pure `lib/download.ts`.
- **Plain vs extended via a query flag.** `GET /api/games/{id}/export?annotated=`
  (verbatim stored PGN vs engine-annotated; annotated needs an engine, 503 else)
  and `GET /api/studies/{id}/export?eval=` (keep vs strip the `[%eval]`
  annotations). Studies acquire evals by importing extended PGN; generated-study
  eval embedding is left out of scope (the round-trip is the deliverable).

**Consequences.** Eval round-trips through `tree_json` and every PGN import/export
with no schema change. Games and studies share exactly one annotation→PGN path, so
they can never drift. The export response shape changed from JSON to a file
download; the only consumer was the (not-yet-surfaced) study store, updated in the
same change.
