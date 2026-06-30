# Architecture Decision Records

Short records of the architectural choices behind chess-base. Add a new file
(`NNNN-title.md`) whenever you make a decision worth remembering. Format:
**Context → Decision → Consequences.**

| # | Decision |
|---|---|
| 0001 | Single crate, not a workspace |
| 0002 | Dual-mode DB: SQLite (local) + Postgres (server) |
| 0003 | Position search via Zobrist-hash index |
| 0004 | Embed the frontend into one binary (rust-embed) |
| 0005 | Engines via auto-download manager |
| 0006 | Frontend board: chessground + chess.js |
| 0007 | Databases are ownable; NULL owner = global |
| 0008 | MCP via hand-rolled JSON-RPC `/mcp` endpoint |
| 0009 | LLM as annotator: the study generation pipeline |
| 0010 | Per-game variant + start position (Chess960-ready) |
| 0011 | Request identity: one `CurrentUser` context, two resolution modes |
| 0012 | UCI engine manager + analysis streamed over a WebSocket |
| 0013 | LLM provider client: `LlmProvider` trait + Anthropic Messages API |
| 0014 | Engine facade: one UCI service, two consumption paths (batch + MCP) |
| 0015 | Server-mode auth: opaque sessions (Bearer/cookie) + first-user-is-admin |
| 0016 | MCP auth & scoping: OAuth 2.1 (server) + service token (local) |
| 0017 | Plan trajectories: pure `plans` module, thin WS/MCP callers |
| 0018 | Pre-chewed DB query layer: ECO + frequency/score + transpositions + references |
| 0019 | Interactive analysis mode: `analyse_position` bundles engine + DB + feature tags |
| 0020 | Incremental sync: persisted cursor + per-game `source_ref` dedup |
| 0021 | Frontend in TypeScript: strict `vue-tsc`, shared `src/types.ts` |
| 0022 | Bulk master-DB PGN import: streaming `.zst`, content-hash dedup, batched txns |
| 0023 | Extended PGN export: `[%eval]` node annotation, one serializer, real `.pgn` downloads |
| 0024 | Toggleable board-overlay layers: static threat scan, one shapes composer, persisted toggles |
| 0025 | Embedded study assistant: in-process agent loop over the MCP registry, approval-gated |
| 0026 | Danger-map opening mode: engine as adjudicator, not author (traps / only-move) |
| 0027 | MCP tools are engine/DB data primitives; the LLM lives on the client side |
| 0028 | Set-up position studies: `start_fen` on the move tree (no migration) |
| 0029 | Plan & threat arrows baked into generated studies: one pass, opt-in, HTTP + `opening_tree` |
| 0030 | Study folder hierarchy (adjacency-list) + game-linked analyses, app-enforced cascade |
