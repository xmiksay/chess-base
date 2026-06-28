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
