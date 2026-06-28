//! Runtime configuration for the HTTP server. Mirrors the config-vs-runtime
//! split: this is parsed once at startup; [`crate::server::AppState`] is the
//! live object handlers use.

use crate::db::DbConfig;
use std::net::IpAddr;

/// Deployment mode. Local = single-user, embedded SQLite, auto-open browser.
/// Server = multi-user, Postgres.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Local,
    Server,
}

/// Fully-resolved server configuration.
#[derive(Debug, Clone)]
pub struct AppConfig {
    pub mode: Mode,
    pub host: IpAddr,
    pub port: u16,
    /// Auto-open the browser after binding (local mode only).
    pub open_browser: bool,
    pub db: DbConfig,
}
