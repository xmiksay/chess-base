//! Shared, cloneable runtime state injected into every handler.

use crate::server::config::Mode;
use sea_orm::DatabaseConnection;

#[derive(Clone)]
pub struct AppState {
    pub db: DatabaseConnection,
    pub mode: Mode,
}
