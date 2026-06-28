//! HTTP server: wiring the router, state, embedded SPA and lifecycle.

pub mod browser;
pub mod config;
pub mod embed;
pub mod routes;
pub mod state;

pub use config::{AppConfig, Mode};
pub use state::AppState;

use anyhow::Result;
use std::net::SocketAddr;

/// Build the Axum router for the given runtime state (used by `serve` and tests).
pub fn build_router(state: AppState) -> axum::Router {
    routes::router(state)
}

/// Connect the database, bind, optionally open the browser, and serve until shutdown.
pub async fn serve(cfg: AppConfig) -> Result<()> {
    let db = crate::db::connect(&cfg.db).await?;
    let state = AppState { db, mode: cfg.mode };
    let app = build_router(state);

    let addr = SocketAddr::new(cfg.host, cfg.port);
    let listener = tokio::net::TcpListener::bind(addr).await?;
    let local = listener.local_addr()?;
    let url = format!("http://{}:{}/", local.ip(), local.port());

    tracing::info!(%url, ?cfg.mode, "chess-base listening");
    println!("\n  chess-base → {url}\n");

    if cfg.open_browser {
        browser::open(&url);
    }

    axum::serve(listener, app).await?;
    Ok(())
}
