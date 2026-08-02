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

## Update (2026-08-02, issue #197) — cursor lifecycle, controls, feedback

**Problem.** The persistence above only covered the happy path:
`ImportService::sync` saved the cursor *after* the collector call succeeded, so
any mid-run failure — a Lichess 429 past its retry budget, a dropped stream, one
bad Chess.com month — discarded the whole run's progress and the next sync
restarted from zero. A history too large to finish in one request (or one prone
to transient failures) could never complete. The cursor was also keyed only by
`(database_id, source)`: syncing a different provider username into the same
collection silently inherited (and could corrupt) another user's cursor. There
was no way to force a full re-sync, and `sync_cursors` carried no
last-synced-at/count, so "0 new games" (incremental worked) and "re-downloaded
and deduped everything" looked identical to a caller.

**Decision.**

- **Collectors report partial progress on failure.** `Lichess::sync` /
  `ChessCom::sync` now return `Result<SyncOutcome, SyncFailure>`;
  `SyncFailure` carries the cursor/imported/duplicates already accumulated
  before the error (`collectors/mod.rs`). `ImportService::sync` persists that
  cursor on *both* branches before mapping a failure to `ImportError::Failed`,
  so a retry resumes from where the failed run stopped instead of restarting.
- **Cursor keyed by `(database_id, source, username)`.** Migration
  `m0009_sync_cursor_lifecycle` adds a nullable `username` column, folded into
  the unique index. `cursor::load`/`save` take the username; a different
  username never resumes from another user's cursor. Pre-#197 rows keep
  `username = NULL` and simply stop matching — a harmless one-off full re-sync
  for that (database, source), safe because of the `source_ref` dedup above.
- **Explicit `full` re-sync.** `ImportService::sync` gains a `full: bool`
  (default `false`): `true` starts from `SyncCursor::default()` instead of the
  stored cursor and overwrites it on completion. Threaded through the HTTP
  `SyncBody`, the MCP `import_sync` tool schema, and a "Full re-sync" checkbox
  in `ImportView.vue`.
- **Real feedback.** `sync_cursors` gains `last_synced_at` / `last_imported` /
  `last_duplicates`, written by every `cursor::save` call. `ImportSummary`
  reports real `duplicates` and a `synced_at` timestamp for a provider sync
  (`None` for a PGN upload); the UI shows "N new · M already present · last
  synced …" instead of a bare game count. The UI also remembers the last sync
  target (`localStorage`) instead of resetting to the first database on mount.

**Consequences.** A large first sync now makes durable progress across retries
instead of looping forever on the same failure. Switching the synced username
for a collection no longer silently skips that user's earlier games. `full`
gives an explicit, discoverable way to force a clean re-sync (e.g. after
suspecting a corrupted cursor) without deleting rows by hand. The username-keyed
index means a collection synced by multiple usernames (rare, but possible)
now gets one cursor row per username rather than one shared row.
