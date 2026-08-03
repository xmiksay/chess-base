//! The `initialize` response's `instructions` text — split out of [`super`]
//! to keep that file under the file-size cap.

/// Returned by `initialize`. Documents the tool surface Epic 9 plugs in and
/// the `<pgn>` / `<fen>` board directives studies render.
pub(super) const INSTRUCTIONS: &str = "\
# chess-base — MCP Integration

Self-hosted ChessBase replacement. Collect, search and study chess games with \
engine analysis and AI-assisted studies. This endpoint exposes chess tooling \
over JSON-RPC; the available tools depend on what is registered (call \
`tools/list`).

## Anonymous access

A request with no `Authorization` header is served as an anonymous public \
caller: data reads only (`list_databases`, `db_list_games`, `db_read_game`, \
`db_position_report`, `db_reference_games`, `db_export_games`, \
`search_headers`, `echo`), scoped to global (admin-managed) databases only — \
no engine, no studies, no writes. `tools/list` reflects this reduced set; any \
other tool returns an authentication-required error. Sign in (OAuth or a \
service token) for your own databases plus write access.

## Tool surface (Epic 9)

- **Interactive analysis** — `analyse_position` is the one-shot \"explain this \
  position\" entry point: it bundles engine eval, the database report and factual \
  feature tags for a single FEN so an explanation cites tool output, not guesses. \
  `analyse_game` is its whole-game counterpart: walk the engine over a PGN for a \
  per-ply eval + best-move + classification review. The tools below are the same \
  sources unbundled, for drilling in further.
- **Engine** — request Stockfish/Lc0 evaluation of a position (best move, score, \
  principal variation) to use as ground truth when annotating.
- **Database** — `list_databases` discovers the collections you can see (with \
  game counts) and the `database_id`s the study tools need; `db_list_games` / \
  `db_read_game` page through and read individual games; `db_position_report` / \
  `db_reference_games` search by position (64-bit Zobrist hash).
- **Study preprocessing** — engine + DB grounded *data* for study building, \
  with no language model inside the tool (you are the model — annotate the \
  output yourself, then persist with the study tools): `opening_tree` builds a \
  pruned, eval- and stats-tagged variation tree (the opening skeleton); \
  `danger_map` walks a repertoire spine PGN into an engine-adjudicated danger \
  tree (Weapon / Caution / Off-book roles); `position_concepts` classifies a \
  position's pawn structure and key squares.
- **Studies** — create studies and edit their move-trees: `study_import_pgn` \
  builds a whole study from PGN in one call, or `study_create` + `study_add_move` \
  (SAN or UCI, with optional inline comment/NAG) build one move at a time; \
  `study_get` reads an existing study's tree (with node ids) so you can \
  `study_annotate` it; `study_export` emits re-importable PGN. Every edit is \
  scoped to the authenticated caller: you may only mutate your own studies \
  (global studies require admin).

## Board directives

When writing study text, embed positions and games with these directives:

- `<fen>FEN string</fen>` — render a static board from an inline FEN.
- `<pgn move=\"N\">PGN moves</pgn>` — render a playable game from inline PGN, \
  opened at half-move N.

Always ground evaluations and variations in the engine and database tools \
rather than asserting them unverified.
";
