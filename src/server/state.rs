//! Shared, cloneable runtime state injected into every handler.

use axum::http::request::Parts;
use sea_orm::DatabaseConnection;

use crate::server::config::Mode;
use crate::server::identity::{AuthError, CurrentUser};

#[derive(Clone)]
pub struct AppState {
    pub db: DatabaseConnection,
    pub mode: Mode,
}

impl AppState {
    /// Resolve the caller's identity for a request — the single seam between the
    /// two run modes. Local mode is always the implicit admin (zero config);
    /// server-mode resolution (session / Bearer auth, possibly a DB lookup)
    /// lands in #14 and only changes this method, not any handler.
    pub async fn resolve_current_user(&self, _parts: &Parts) -> Result<CurrentUser, AuthError> {
        match self.mode {
            Mode::Local => Ok(CurrentUser::local_admin()),
            Mode::Server => Err(AuthError::Unauthorized),
        }
    }
}
