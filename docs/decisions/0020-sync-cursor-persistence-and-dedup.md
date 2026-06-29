# 0020 — Incremental sync: persisted cursor + per-game dedup

**Context.** Each collector (`lichess`, `chesscom`) advances a `SyncCursor`, but
`ImportService::sync` discarded it and started every run from `SyncCursor::default()`,
and neither collector deduped. So every re-sync re-downloaded and re-stored the
whole history — doubling `games` and `position_index` rows and corrupting the
opening-tree / frequency stats that key on those rows. Two boundary bugs were
latent behind the missing persistence: Chess.com's `months_to_sync` used `key > last`
(skipping the rest of the last-synced month forever) and Lichess's `since_param`
used `last_game_ms + 1000` (skipping games sharing the boundary second).

**Decision.** Persist the cursor and dedup by a stable game key, which makes
re-syncing the boundary safe:

- **Cursor persistence.** A `sync_cursors` table holds one row per
  `(database_id, source)` (unique index). `ImportService::sync` loads it before the
  collector run and saves the advanced cursor after (`imports/cursor.rs`).
- **Dedup key.** `games.source_ref` stores the provider permalink — Lichess's
  `[Site]` URL or Chess.com's `[Link]` — unique per `database_id`. `ingest_pgn`
  skips a game whose `(database_id, source_ref)` already exists, returning
  `Ok(None)`. Games without a permalink (manual uploads) keep `source_ref = NULL`
  and are never deduped (NULL is distinct in a unique index on both backends).
- **Resume *at* the boundary, not past it.** With dedup in place the collectors
  re-fetch the boundary deliberately: Chess.com re-syncs the cursor month (`>=`),
  Lichess resumes at the last game's second (no `+1s` nudge). The already-stored
  games are deduped; genuinely new games in that month/second are no longer lost.

**Consequences.** Re-syncs are incremental and idempotent: only new games are
stored, opening/frequency stats stay correct. Dedup is a cheap indexed lookup per
game plus a unique-index backstop. The key is the provider permalink, so historic
PGNs without one (and intentional manual re-uploads) are unaffected. Migration
`m0005_sync_dedup` adds the column, the `sync_cursors` table and both unique indices.
