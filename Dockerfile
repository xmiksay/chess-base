# syntax=docker/dockerfile:1
#
# Multi-stage build for the chess-base server.
#   1. node    — build the Vue SPA into frontend/dist.
#   2. rust    — cargo build --release with the SPA embedded (rust-embed).
#   3. runtime — slim Debian image with just the binary + CA certs.
#
# The result runs in **server mode** (Postgres, multi-user); migrations run
# automatically on startup. See docker-compose.yml for the full stack.

# ---- 1. Frontend ------------------------------------------------------------
FROM node:22-slim AS frontend
WORKDIR /app/frontend
# Install deps first so the layer caches when only sources change.
COPY frontend/package.json frontend/package-lock.json ./
RUN npm ci
COPY frontend/ ./
RUN npm run build

# ---- 2. Backend -------------------------------------------------------------
FROM rust:1-slim-bookworm AS backend
# zstd-sys (and other -sys crates) compile C, so a toolchain is required.
RUN apt-get update \
    && apt-get install -y --no-install-recommends build-essential pkg-config \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /app
ENV CARGO_BUILD_JOBS=4
# Cache the dependency build: compile against a stub main, then the real source.
COPY Cargo.toml Cargo.lock build.rs ./
RUN mkdir -p src/bin frontend/dist \
    && echo "fn main() {}" > src/bin/chess-base.rs \
    && echo "" > src/lib.rs \
    && cargo build --release --locked --bin chess-base \
    ; rm -rf src
# Real sources + the built SPA (embedded at compile time by rust-embed).
COPY src ./src
COPY assets ./assets
COPY --from=frontend /app/frontend/dist ./frontend/dist
# Bust the stub's cached crate so the real binary is rebuilt.
RUN touch src/bin/chess-base.rs src/lib.rs \
    && cargo build --release --locked --bin chess-base

# ---- 3. Runtime -------------------------------------------------------------
FROM debian:bookworm-slim AS runtime
# CA certs are needed for outbound TLS (Lichess / Chess.com collectors).
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*
# Run as a non-root user.
RUN useradd --create-home --uid 10001 chess
USER chess
WORKDIR /home/chess
COPY --from=backend /app/target/release/chess-base /usr/local/bin/chess-base

EXPOSE 3030
# Args (server mode, bind address, port, database URL) are supplied by the
# compose `command:`; DATABASE_URL may also come from the environment.
ENTRYPOINT ["chess-base"]
CMD ["--server", "--host", "0.0.0.0", "--port", "3030"]
