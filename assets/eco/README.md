# ECO opening dataset

`a.tsv` … `e.tsv` are the [lichess-org/chess-openings](https://github.com/lichess-org/chess-openings)
dataset (public domain, CC0). Each row is `eco<TAB>name<TAB>pgn`, where `pgn` is
the mainline leading to the named opening position.

Embedded into the binary by [`src/openings.rs`](../../src/openings.rs), which
replays each `pgn` to derive the position's Zobrist hash and builds an O(1)
`zobrist -> (eco, name)` lookup. To refresh, re-download the upstream `*.tsv`
files into this directory.
