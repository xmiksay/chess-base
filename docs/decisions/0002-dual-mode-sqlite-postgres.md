# 0002 — Dual-mode database: SQLite (local) + Postgres (server)

**Context.** The app must run two ways: locally like the `design` app (one binary,
embedded DB, opens a browser) and as a multi-user server.

**Decision.** Use **SeaORM** over sqlx with both the `sqlx-sqlite` and
`sqlx-postgres` backends compiled in. `DbConfig`/`Backend` (`src/db/config.rs`)
selects the backend at runtime; the same migrations (schema builder, not raw SQL)
run on both. Local mode uses SQLite; `--server` uses Postgres.

**Consequences.** One codebase, one binary, both deployments. Migrations must stay
within the portable subset of the schema builder. In-memory SQLite (tests) needs a
single pooled connection, handled in `db::connect`.
