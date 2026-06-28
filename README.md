# chess-base

A self-hosted **ChessBase replacement** — Rust backend + Vue 3 frontend to
collect, store, search and study chess games, with engine analysis and
AI-assisted studies.

## Features (roadmap)

- Collect games from **Lichess** and **Chess.com**.
- Import **master game databases** (bulk PGN / `.zst`).
- Store games and **search by header *and* by board position** (Zobrist index).
- **Studies**: commented PGN with variations (the move-tree editor).
- **Engine analysis**: Stockfish and Lc0/**Maia** over UCI (auto-downloaded).
- Optional **multi-user** server mode with logins and shared databases.
- **MCP endpoint** so an AI agent can build and annotate studies.

## Run modes

| Mode | DB | Users | Browser | Command |
|---|---|---|---|---|
| Local | SQLite (file) | single (admin) | auto-opens | `make run` |
| Server | Postgres | multi-user | no | `chess-base --server --database-url postgres://…` |

## Quick start (local)

```sh
# Node 22 (frontend) + Rust toolchain required
nvm use            # in frontend/, honors frontend/.nvmrc
make deps          # install frontend deps (first time)
make run           # builds the SPA, runs locally, opens your browser
```

## Development

```sh
make dev           # backend on :3030 + Vite hot-reload (proxies /api)
make test          # Rust unit + integration + frontend tests
make coverage      # cargo llvm-cov + vitest coverage
make lint          # clippy -D warnings + fmt --check + eslint
make help          # list all targets
```

## Layout

```
src/            Rust backend (single crate; see .claude/CLAUDE.md)
frontend/       Vue 3 + Vite + Tailwind + chessground
docs/           architecture.md + decisions/ (ADRs)
```

See [`docs/architecture.md`](docs/architecture.md) for the full design and
[`docs/decisions/`](docs/decisions/) for the rationale behind key choices.

## License

MIT
