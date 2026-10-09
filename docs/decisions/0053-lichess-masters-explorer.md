# ADR-0053 — Lichess Masters as a remote explorer source

Status: accepted

## Context

The position explorer only searched the caller's own databases. Master-level
reference stats belong in the explorer too, but the local database is meant to
stay **slim**, holding only the owner's games and studies (ADR-0052). Bulk-importing
a masters dump (the `import-pgn` CLI) would mean millions of rows and a large
`position_index`. Lichess already serves the masters database through its opening
explorer (`explorer.lichess.ovh/masters`), which now requires an authenticated
request.

## Decision

1. **Masters is a remote reference.** `src/explorer/` proxies
   `GET /masters?fen&since&until` through the backend (`MastersClient`). Masters
   games are only stored when a user explicitly imports one.
2. **Token-gated.** `LICHESS_TOKEN` / `--lichess-token` is sent as a Bearer token.
   With no token, `AppState.masters` is `None`, the routes return `503`,
   `/api/health` reports `masters: false` and the SPA hides the tab.
3. **Signed-in callers only.** The HTTP routes use the `CurrentUser` extractor, so
   an anonymous server-mode request gets `401`. The MCP tool is off the anonymous
   allowlist and refuses a `public` caller. This keeps the shared token's rate
   limit from becoming a public resource.
4. **In-memory cache.** Answers are cached per `(fen, since, until)` for 24h, up to
   4096 entries (expired entries are evicted first, then the oldest). On any
   upstream failure a stale entry is served instead of an error. A Lichess `429`
   stops all upstream calls for 60s, as Lichess requires.
5. **Endpoints.**
   - `GET /api/explorer/masters?fen&since&until` returns
     `{ total, white, draws, black, opening, moves[], top_games[] }`. Each entry in
     `moves` has the local `MoveStat` shape (`san`/`count`/`white`/`draws`/`black`)
     plus `uci` and `average_rating`, so both tabs render with one table.
   - `POST /api/explorer/masters/import {lichess_id, database_id}` fetches
     `/masters/pgn/{id}` (the id is validated as a short alphanumeric before it is
     put into a URL) and runs it through `ImportService::import_pgn`. That gives the
     usual write guard and dedup by source ref.
6. **Filters: year from/to only.** Lichess Masters doesn't support player or colour
   filters, so the Masters tab hides them.
7. **UI.** A "My databases / Lichess Masters" toggle in `PositionExplorer.vue`
   switches source while keeping the same board line. `MastersTopGames.vue` lists
   the top games, each importable into a writable database the user picks.
   "Add line to study" cites the active tab's stats; for Masters the comment reads
   "Masters: N games, W/D/L". Switching source drops the captured stat rather than
   citing it under the wrong label.
8. **MCP.** A read-only `masters_position_report` tool mirrors the GET route. The
   import is on the symmetry carve-out list: pulling a remote game into the local
   DB is a deliberate UI action.
   *Update (ADR-0057):* the agent now imports a single game, Masters or
   lichess.org, through the gated `import_lichess_game` tool
   (`POST /api/import/lichess-game`). This route stays as its Masters-only twin.

## Consequences

- A masters lookup depends on Lichess being reachable. The stale cache only covers
  positions already seen.
- The cache is per process and is lost on restart, which is fine for a
  reference-only feature.
- Every signed-in user shares one token's rate budget. The cache and the 429
  back-off keep usage modest for a personal deployment.
