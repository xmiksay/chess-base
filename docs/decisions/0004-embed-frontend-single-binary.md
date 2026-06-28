# 0004 — Embed the frontend into one binary (rust-embed)

**Context.** Local mode should be a single, self-contained executable that opens a
browser — no separate static file server or Node runtime at run time.

**Decision.** Build the Vue SPA to `frontend/dist` and embed it with `rust-embed`
(`src/server/embed.rs`); the Axum fallback handler serves embedded assets with an
`index.html` SPA fallback. `build.rs` ensures `frontend/dist` exists so the crate
compiles even before the SPA is built; `make build`/CI build the SPA first.

**Consequences.** Distribution is one binary. The browser is opened via the `open`
crate in local mode (the URL is also printed). Dev uses Vite's server with an
`/api` proxy instead of the embedded assets.
