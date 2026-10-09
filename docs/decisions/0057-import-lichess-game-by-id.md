# ADR-0057 — Import one Lichess game by id (lichess.org or Masters)

Status: accepted (updates ADR-0053 §8)

## Context

The in-app agent could not import a single Lichess game. `import_pgn` needs
the PGN text, which the agent cannot fetch. `import_sync` pulls a player's
whole history. `masters_position_report` hands back `top_games` ids, but the
only route that imports one (`POST /api/explorer/masters/import`) was an
HTTP-only carve-out (ADR-0053 §8). A regular lichess.org game had no
single-game import at all, over HTTP or MCP.

## Decision

1. **One service, `ImportService::import_lichess_game`** (`imports/lichess_game.rs`).
   It takes a game id or URL, a target database and the optional `MastersClient`,
   and checks the write guard before any network call.
2. **Id parsing is pure and strict.** `parse_game_ref` accepts a bare id,
   `lichess.org/{id}`, `/{id}/black`, `#ply`, a 12-char player id (cut to the
   8-char game id), `/game/export/{id}` and `explorer.lichess.ovh/masters/pgn/{id}`.
   A URL on another host, or a profile URL (`/@/name`), is a 400. Only a plain
   alphanumeric id reaches an upstream URL.
3. **lichess.org first, Masters on a 404.** A bare id doesn't say which id space
   it belongs to. `GET lichess.org/game/export/{id}?clocks=false&evals=false`
   is tried first. Only a 404 falls back to `MastersClient::game_pgn`. A 429 or
   any other upstream failure is reported as-is and never retried against Masters.
   Without `LICHESS_TOKEN` (no Masters client), a lichess.org miss says the
   Masters lookup needs the token.
4. **Surfaces.**
   - `POST /api/import/lichess-game {database_id, game}`. The response is the
     shared import summary (`imported`, `duplicates`, `game_ids`, …). Failures:
     400 for a bad id, not found or upstream failure; 403 forbidden; 404 for an
     unknown database.
   - MCP/agent tool `import_lichess_game`, mirrored in `symmetry.rs` and **gated**
     in `GATED_TOOLS` (it writes).
   - "Import a Lichess game" form in `ImportView`.
5. **`POST /api/explorer/masters/import` stays** for `MastersTopGames.vue`. It
   remains a symmetry carve-out, now because it is the Masters-only twin of the
   new route, which MCP already mirrors.

## Consequences

- The agent can chain `masters_position_report` → `import_lichess_game` →
  `analyse_game` / `game_save_as_study` without user copy-paste.
- A Masters game costs two upstream calls (the lichess.org 404, then Masters).
  That is acceptable for a deliberate single-game import.
- The local DB stays slim (ADR-0052): games still come in one deliberate import
  at a time, never in bulk.
