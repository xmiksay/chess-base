//! HTTP server: wiring the router, state, embedded SPA and lifecycle.

pub mod auth;
pub mod browser;
pub mod config;
pub mod embed;
pub mod engine_ws;
pub(crate) mod error;
pub mod identity;
pub mod routes;
pub mod state;

pub use config::{AppConfig, Mode};
pub use identity::{assert_admin, scope, AuthError, CurrentUser};
pub use state::AppState;

use anyhow::Result;
use std::net::SocketAddr;
use std::sync::Arc;

use crate::engine::{download_default_engines, EngineRegistry, EngineService};

/// Engines in the pooled facade. One keeps batch + MCP analysis serialized so a
/// multi-threaded engine isn't oversubscribed against itself on a shared host.
const ENGINE_POOL_SIZE: usize = 1;

/// Build the Axum router for the given runtime state (used by `serve` and tests).
pub fn build_router(state: AppState) -> axum::Router {
    routes::router(state)
}

/// Connect the database, bind, optionally open the browser, and serve until shutdown.
pub async fn serve(cfg: AppConfig) -> Result<()> {
    let db = crate::db::connect(&cfg.db).await?;

    // Seed the operator-supplied `--engine` into the persistent registry, then
    // resolve the effective default. A persisted user selection wins across
    // restarts; the CLI flag only fills in when nothing is configured yet.
    let registry = EngineRegistry::new(db.clone());
    if let Some(engine) = cfg.engine.clone() {
        registry.seed_default(engine).await?;
    }
    // First-run auto-download (ADR 0005 / #11): when enabled and nothing is
    // configured yet, fetch Stockfish + Maia into the engines dir and register
    // them in the lowest-priority resolution slot. Best-effort — a download or
    // checksum failure is logged and the server still starts (just without a
    // default engine), never panics.
    if cfg.download_engines && registry.resolve_default().await?.is_none() {
        match download_default_engines(&cfg.engines_dir).await {
            Ok(engines) if !engines.is_empty() => {
                tracing::info!(count = engines.len(), "auto-downloaded engines");
                if let Err(e) = registry.set_downloaded(&engines).await {
                    tracing::warn!(error = %e, "could not record auto-downloaded engines");
                }
            }
            Ok(_) => {}
            Err(e) => {
                tracing::warn!(error = %format!("{e:#}"), "engine auto-download failed; continuing without a default engine")
            }
        }
    }
    let default_engine = registry.resolve_default().await?;

    // One pool backs both facades: the direct batch API and the MCP tool.
    let engine_service = default_engine.map(|c| Arc::new(EngineService::new(c, ENGINE_POOL_SIZE)));
    let state = AppState {
        db: db.clone(),
        mode: cfg.mode,
        engine_service,
    };
    let app = build_router(state);

    let addr = SocketAddr::new(cfg.host, cfg.port);
    let listener = tokio::net::TcpListener::bind(addr).await?;
    let local = listener.local_addr()?;
    let url = format!("http://{}:{}/", local.ip(), local.port());

    tracing::info!(%url, ?cfg.mode, "chess-base listening");
    println!("\n  chess-base → {url}\n");

    // Local mode is gated by a printed service token: provision (or reuse) it and
    // print the line that wires this instance into Claude over the MCP transport.
    if cfg.mode == Mode::Local {
        match auth::ensure_local_service_token(&db).await {
            Ok(token) => {
                println!("  MCP service token: {token}");
                println!("  Connect Claude to this instance:");
                println!(
                    "    claude mcp add --transport http chess-base {url}mcp \
                     --header \"Authorization: Bearer {token}\"\n"
                );
            }
            Err(e) => tracing::warn!(error = %e, "could not provision local MCP service token"),
        }
    }

    if cfg.open_browser {
        browser::open(&url);
    }

    axum::serve(listener, app).await?;
    Ok(())
}
