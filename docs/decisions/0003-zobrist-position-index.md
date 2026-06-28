# 0003 — Position search via a Zobrist-hash index

**Context.** The hard requirement is searching games **by board position**, not
just by header metadata. Lichess's opening explorer uses RocksDB keyed by Zobrist
hash at trillion-position scale — more than a self-hosted instance needs.

**Decision.** Hash every position with the 64-bit Polyglot-compatible Zobrist key
from `position::zobrist_of_fen` (shakmaty). Store a position index row
`(zobrist, game_id, ply, move, database_id)` per mainline ply; answer
"games reaching this position / move stats from here" via an indexed lookup on
`zobrist`. A plain indexed integer column works identically on SQLite and Postgres.

**Scale — per-database index depth.** One row per ply per game is fine for a user's
own databases (their Lichess/Chess.com games — thousands of games, well under a
million rows), but a master database is ~11M games × ~80 plies ≈ 900M rows —
impractical for local SQLite. The opening-explorer value is concentrated in the
opening anyway. So each `databases` row carries an **`index_depth`** policy:
`NULL` = full per-ply indexing (default for own/lichess/chesscom DBs); an integer
caps indexing to the first N plies (default ~36 for `master`/global DBs, ≈400M
rows instead of 900M). The ingest pipeline stops writing `position_index` rows past
the cap. Position search over a capped database therefore covers openings and early
middlegame; this limit is surfaced in the UI so deep master queries don't look
silently empty. Own databases stay fully searchable.

**Consequences.** No separate key-value store; reuses the relational DB and the
pure `position` module. The index grows ~one row per ply per game up to the
per-database depth cap. Hash collisions are astronomically unlikely but candidates
can be verified against the stored game if ever needed.
