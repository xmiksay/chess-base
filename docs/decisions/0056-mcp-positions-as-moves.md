# ADR-0056 — MCP position tools accept `moves`, and echo the resolved FEN

Status: accepted

## Context

Every position-taking MCP tool only accepted a `fen`. Models write FEN badly.
In one assistant session (GLM via the in-app assistant) every FEN the model
built past move 2 was broken: wrong side to move, an impossible en-passant
square, a pawn missing or doubled. `masters_position_report` answered those
with `total: 0` or `illegal position`. The model then told the user that the
Lichess Masters feed "only serves 2 plies", and said it had double-checked its
FENs. The tool worked; the input was wrong and nothing pointed at that.

## Decision

1. **`moves` alternative.** `masters_position_report`, `db_position_report`,
   `db_reference_games`, `analyse_position`, `engine_analyse`,
   `position_concepts`, `position_threats`, `opening_tree` and `danger_map`
   accept `moves`: a SAN array played from the start position, or from `fen`
   when both are given. The server replays them
   (`server/routes/mcp/position_arg.rs`). An illegal move is the caller's
   error and names the move and its index, so a bad line fails loudly
   instead of returning an empty answer. The HTTP API is unchanged; the SPA
   always sends exact FENs.
2. **`fen` is no longer `required` in the schemas.** JSON Schema can't express
   "one of `fen`/`moves`" in a form every provider accepts.
   `require_position` enforces it at call time.
3. **Echo the FEN.** Single-position tools return the resolved `fen` so the
   model can see and reuse it. Object outputs gain a `fen` key. The two
   array outputs change shape: `db_reference_games` → `{fen, games}`,
   `position_threats` → `{fen, shapes}`.
4. **Guidance in the tool description.** The Masters → study workflow
   (branching budget, stats-in-comments PGN, `study_import_pgn` →
   `study_analyse` → `study_annotate`, "`total: 0` is not an outage") lives in
   `masters_position_report`'s description, which the in-app assistant and
   external MCP clients both read. The system prompt and the MCP
   `instructions` only point to it.

## Consequences

- Breaking for MCP clients that parsed the old array outputs of
  `db_reference_games` / `position_threats`.
- `preprocess.rs`'s tests moved to `preprocess_tests.rs` so the file could
  take the change without growing past the size cap.
