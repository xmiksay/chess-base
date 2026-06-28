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

Point `--engine <path>` (or `CHESS_BASE_ENGINE`) at a UCI engine binary (e.g.
Stockfish) to enable live analysis over the `/api/engine/analyse` WebSocket;
`--engine-weights` supplies an Lc0/Maia net. Without it the route returns `503`.

## Quick start (local)

### Download a release (no toolchain needed)

Grab the archive for your platform from the
[latest release](https://github.com/xmiksay/chess-base/releases/latest)
(linux x86_64, macOS arm64/x86_64, Windows x86_64), extract it, and run the
single self-contained binary — it starts in **local mode**, creates the SQLite
database, and opens your browser:

```sh
./chess-base          # Windows: chess-base.exe
```

### Build from source

```sh
# Node 22 (frontend) + Rust toolchain required
nvm use            # in frontend/, honors frontend/.nvmrc
make deps          # install frontend deps (first time)
make run           # builds the SPA, runs locally, opens your browser
make release       # build the locked, self-contained release binary
```

Tagging `vX.Y.Z` runs the [release workflow](.github/workflows/release.yml),
which builds the SPA, embeds it, and publishes a binary per platform.

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
