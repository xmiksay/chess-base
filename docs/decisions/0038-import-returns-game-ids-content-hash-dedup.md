# 0038 — PGN import returns the created game ids; content-hash dedup on every import path

**Context.** `import_pgn` is the only way to create a game — over HTTP *and*
over MCP (ADR-0036 kept the surfaces symmetric; there is deliberately no
single-game "create" endpoint). Tracing the MCP "create a game" flow exposed
three defects that together made it unusable for an agent:

1. The response (`{ imported, skipped, errors }`) dropped the new games' ids,
   even though `IngestReport` already carried them — a client that just created
   a game could not chain it into `db_read_game` / `analyse_game` /
   `game_save_as_study` without guessing via `db_list_games` (whose default
   date sort hides a hand-written game with a placeholder date).
2. A duplicate permalink was silently dropped (`ingest_pgn` → `Ok(None)`), so
   re-importing an existing game returned `{ imported: 0, skipped: 0,
   errors: [] }` — success with nothing created and no explanation.
3. Content-hash dedup existed (`ParsedGame::content_hash`, issue #4) but only
   the bulk importer used it: re-uploading the same permalink-less PGN via
   HTTP/MCP duplicated the game every time — inconsistent with both the
   permalink dedup (issue #95) and the bulk path.

**Decision.**

- **Every game gets a `source_ref`.** `ingest_pgn` falls back to
  `ParsedGame::content_hash` when there is no provider permalink, checks it
  before the replay, and stores it — the same key/behavior as the bulk
  importer. Manual re-uploads now dedup per database like synced games do.
  Rows ingested before this change keep `source_ref NULL` and are not
  retro-deduped.
- **Duplicates are counted, not swallowed.** `IngestReport` gains
  `duplicates`; `ImportSummary` and both wire shapes (HTTP
  `POST /api/import/pgn` and the MCP `import_pgn` tool) gain `duplicates`, so
  `imported: 0, duplicates: 1` reads as "already there" instead of a silent
  no-op. The SPA import view surfaces the count per job and in the summary.
- **The import response carries `game_ids`** — the newly created games' ids in
  PGN order — so a client (human or agent) can chain the created game into
  further calls. A provider `sync` stays counts-only (`game_ids` empty): it is
  bulk-scale and its cursor-boundary dedup is the intended behavior (#95).

**Consequences.** Creating a game over MCP is now a two-field read:
`game_ids[0]` to continue working with it, `duplicates` to explain an empty
result. Uploads are idempotent per database. A legitimate re-play of the exact
same moves under identical headers must differ in some header (e.g. `[Round]`)
to be stored twice — the same trade-off the bulk importer already made.
