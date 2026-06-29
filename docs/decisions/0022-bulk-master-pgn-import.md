# 0022 — Bulk master-DB PGN import (streaming, content-hash dedup)

**Context.** Free master sources (Lumbras Giga Base, Ajedrez Data / TWIC) ship
plain PGN or `.pgn.zst` with millions of games (issue #4). The existing
`ingest_pgn_all` path reads the whole blob into memory, opens one transaction per
game, and dedups only on a provider permalink (`source_ref`) — which master games
do not carry. None of that survives a multi-GB file: memory blows up, per-game
commits are slow under SQLite, and a re-run after an interruption re-imports
everything.

**Decision.** A dedicated `collectors::bulk::BulkImporter` reusing the ingest
seams rather than a parallel store path:

- **Streaming, bounded memory.** The file is read in fixed 64 KiB chunks into a
  buffer; whenever a second `[Event ` boundary appears, every provably-complete
  game before it is drained and processed, leaving only the trailing partial game
  buffered (the same boundary logic the Lichess collector streams with). Peak
  memory is one in-flight game + one chunk + the current batch.
- **`.zst` transparently.** A `.zst` path is wrapped in a `zstd` streaming
  decoder; any other extension is read as plain PGN. The core takes any
  `io::Read`, so tests drive it from an in-memory cursor.
- **Batched transactions.** `ingest.rs` is factored into reusable seams —
  `parse_pgn`, `prepare_game`, `load_index_depth`, `store_prepared` — so the
  importer validates each game and writes many per transaction (default 1000),
  amortizing commit cost. `ingest_pgn` now calls the same seams (one game/txn).
- **Content-hash dedup, restartable.** Master games have no permalink, so
  `ParsedGame::content_hash` (SHA-256 over the normalized roster, variant/start
  position and mainline SAN, `sha256:`-prefixed) becomes the game's `source_ref`.
  The existing unique `(database_id, source_ref)` index then makes a re-run skip
  everything already imported — the import is restartable with no new schema. A
  per-run `HashSet` also drops duplicates that appear within one file.
- **SQLite bulk PRAGMAs.** Before importing, the SQLite connection gets
  `journal_mode=WAL`, `synchronous=NORMAL`, `temp_store=MEMORY` and a ~64 MiB
  page cache (no-op on Postgres).
- **Entry point.** A `chess-base import-pgn <FILE>` CLI subcommand connects,
  resolves the global `master` database via `find_or_create_master`, runs the
  importer and prints the tally — no server started.

**Consequences.** No migration: dedup rides the `source_ref` column already added
by `m0005_sync_dedup`, with `sha256:` keeping content hashes disjoint from URL
permalinks. Reusing `store_prepared` keeps bulk and single-game ingest identical
in how games are stored and indexed (DRY). Bad/illegal games are skip-and-continue
(counted, not fatal); only a storage failure aborts. The content hash dedups
*identical* games — two genuinely distinct games sharing headers and moves would
collide, which is the intended behaviour for a master base. Per-connection PRAGMAs
are best-effort on a pool, which is fine for this sequential admin path.
