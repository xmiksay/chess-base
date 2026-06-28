# 0003 — Position search via a Zobrist-hash index

**Context.** The hard requirement is searching games **by board position**, not
just by header metadata. Lichess's opening explorer uses RocksDB keyed by Zobrist
hash at trillion-position scale — more than a self-hosted instance needs.

**Decision.** Hash every position with the 64-bit Polyglot-compatible Zobrist key
from `position::zobrist_of_fen` (shakmaty). Store a position index row
`(zobrist, game_id, ply, move, database_id)` per mainline ply; answer
"games reaching this position / move stats from here" via an indexed lookup on
`zobrist`. A plain indexed integer column works identically on SQLite and Postgres.

**Consequences.** No separate key-value store; reuses the relational DB and the
pure `position` module. The index grows ~one row per ply per game — acceptable at
our scale. Hash collisions are astronomically unlikely but candidates can be
verified against the stored game if ever needed.
