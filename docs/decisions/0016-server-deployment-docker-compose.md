# 0016 — Server deployment: Docker image + Compose with Postgres

## Status

Accepted.

## Context

Server mode (ADR 0002) runs multi-user against Postgres. Local mode ships as a
self-contained release binary (ADR 0004), but the server deployment needed a
reproducible, one-command story that mirrors the workspace Docker/Compose
convention. Migrations already run automatically on startup
(`db::connect` → `Migrator::up`), so deployment only needs to provide the binary
and a database.

## Decision

- **Multi-stage `Dockerfile`**: a `node:22-slim` stage builds the Vue SPA, a
  `rust:1-slim-bookworm` stage compiles the release binary with the SPA embedded
  (rust-embed), and a `debian:bookworm-slim` runtime stage carries only the
  binary plus `ca-certificates` (needed for the outbound-TLS collectors). The
  runtime runs as a non-root user; `ENTRYPOINT` is the binary, default `CMD`
  selects server mode bound to `0.0.0.0:3030`.
- **`docker-compose.yml`** wires the app to a `postgres:16-alpine` service with a
  named `pgdata` volume and a `pg_isready` healthcheck; the app waits on
  `service_healthy` and reads `DATABASE_URL` (the CLI's `clap(env)`) pointed at
  the `db` service. Credentials and the published port come from `.env`
  (`.env.example` is the template).

The container build pre-compiles dependencies against a stub crate before
copying real sources so dependency layers cache across source changes. TLS is
rustls throughout (no OpenSSL), so the runtime image needs no native crypto
libraries.

## Consequences

- `docker compose up --build` brings up Postgres + the app, runs migrations, and
  serves the multi-user app at `http://localhost:${APP_PORT}`.
- The image is server-mode only; local mode stays the release-binary path.
- A UCI engine is not bundled; live analysis in the container requires mounting
  an engine binary and setting `CHESS_BASE_ENGINE` (otherwise the analysis route
  returns `503`, as in any deployment without an engine).
