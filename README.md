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

### Bulk-import a master database

To load a large master collection (e.g. Lumbras Giga Base or TWIC, plain `.pgn`
or `.pgn.zst`) into a global **master** database without starting the server:

```sh
chess-base import-pgn games.pgn.zst            # into chess-base.db, "Master Database"
chess-base import-pgn games.pgn.zst --name "TWIC" --batch-size 2000
```

The file is streamed in bounded memory (`.zst` is decompressed on the fly),
games are committed in batched transactions, and duplicates are skipped by a
content hash — so an interrupted import can simply be re-run to resume.

On first run, with no engine configured, chess-base **auto-downloads** Stockfish
(and Lc0 + Maia weights where available) into `engines/` — verifying each file's
checksum — so live analysis works out of the box. Override the directory with
`--engines-dir` / `CHESS_BASE_ENGINES_DIR`, or disable it with
`--no-engine-download` (downloads are best-effort; a failure is logged, not fatal).

To use an engine you already have, point `--engine <path>` (or `CHESS_BASE_ENGINE`)
at a UCI engine binary (e.g. Stockfish) for live analysis over the
`/api/engine/analyse` WebSocket; `--engine-weights` supplies an Lc0/Maia net. The
flag seeds a persisted **engine registry**: manage several engines and pick the
default from the Settings panel or the `/api/engines` API (each engine takes an
optional `runner` wrapper, e.g. `wine`). A user-set engine always wins over an
auto-downloaded one; with nothing configured at all the route returns `503`.

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

### Offline builds: bundled Stockfish (opt-in)

For **air-gapped / offline** local builds, an opt-in `bundled-stockfish` Cargo
feature embeds a per-target Stockfish binary so live analysis works with no
`--engine` and no download — it is extracted to the OS cache dir on first run and
registered as the default engine (below any user override, above auto-download).
The default build embeds nothing, so its size and licensing are unaffected.

```sh
make build-bundled     # fetch this host's Stockfish, embed it, build the release
# equivalently:
make bundle-stockfish  # fetch into engines-bundled/<target>/ (network, once)
cargo build --release --features bundled-stockfish   # then builds fully offline
```

The build **checksum-verifies** the embedded binary; a mismatch fails the build.

> **LICENSING — GPLv3.** Stockfish is licensed under the **GPLv3**. Enabling
> `bundled-stockfish` embeds Stockfish into the binary, so **that build artifact
> is GPLv3** and must be distributed accordingly. The default download build is
> **unaffected** (a separately-fetched child process is mere aggregation). The
> feature is never silently on — it requires this explicit opt-in. Lc0/Maia are
> never bundled (large weights, per-target binaries — download only).

## Server deployment (Docker + Postgres)

Server mode runs multi-user against Postgres. The repo ships a multi-stage
`Dockerfile` and a Compose stack (app + Postgres + named volume):

```sh
cp .env.example .env     # set POSTGRES_PASSWORD (and optionally APP_PORT)
docker compose up --build
```

This builds the SPA, compiles the release binary with it embedded, starts
Postgres, **runs migrations automatically** on startup, and serves the app at
`http://localhost:${APP_PORT}` (default `3030`). Game data persists in the
`pgdata` volume. The image **bundles Stockfish** (`bundled-stockfish` feature),
so live analysis works out of the box with no download — which also makes the
image a **GPLv3 artifact** (see the licensing note above). See
[ADR-0016](docs/decisions/0016-server-deployment-docker-compose.md) and
[ADR-0037](docs/decisions/0037-k8s-deployment-ghcr-bundled-image.md).

## Deploy (k8s)

Version tags (`v*`) build the image via `.github/workflows/docker.yml` and
publish it to **`ghcr.io/xmiksay/chess-base`** (a **public** GHCR package —
make it public in the GitHub package settings after the first push) tagged
with the semver, the short commit SHA and `latest`. `deploy.yml` is a
single-file manifest (Secret, ConfigMap, Deployment, Service, Ingress)
targeting the `services` namespace with the shared Postgres; it pins the
image by tag — bump the tag there to roll a new release:

```sh
# one-time: create the DB in the shared Postgres
kubectl -n services exec deploy/postgres -- psql -U postgres -c \
  "CREATE ROLE chessbase LOGIN PASSWORD '<pw>'; CREATE DATABASE chessbase OWNER chessbase;"
# set the real DATABASE_URL in deploy.yml's Secret (placeholder is CHANGE_ME), then:
make deploy            # kubectl apply -f deploy.yml
make deploy-restart    # later: roll pods onto the freshly pushed :main image
```

The app serves at `https://chessbase.mmik.cz` (nginx ingress + cert-manager).
The pod's **CPU limit (2)** is the guard against heavy Stockfish analysis
starving the node. See
[ADR-0037](docs/decisions/0037-k8s-deployment-ghcr-bundled-image.md).

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
