//! Shared, cloneable runtime state injected into every handler.

use std::sync::Arc;

use axum::http::request::Parts;
use sea_orm::DatabaseConnection;

use crate::engine::{EngineConfig, EngineService};
use crate::server::config::Mode;
use crate::server::identity::{AuthError, CurrentUser};

#[derive(Clone)]
pub struct AppState {
    pub db: DatabaseConnection,
    pub mode: Mode,
    /// The engine the analysis WebSocket spawns per socket, if one is
    /// configured. `None` disables `/api/engine/analyse` (it answers `503`).
    pub engine: Option<EngineConfig>,
    /// Pooled one-shot engine facade backing the batch `analyse` API and the MCP
    /// `engine_analyse` tool. Built from the same `EngineConfig`; `None` ⇒ those
    /// paths are disabled.
    pub engine_service: Option<Arc<EngineService>>,
}

impl AppState {
    /// Resolve the caller's identity for a request — the single seam between the
    /// two run modes. Local mode is always the implicit admin (zero config);
    /// server mode reads the session token (Bearer header or `session` cookie)
    /// and resolves it through [`AuthService`]. Only this method differs between
    /// modes; no handler signature does.
    ///
    /// [`AuthService`]: crate::auth::AuthService
    pub async fn resolve_current_user(&self, parts: &Parts) -> Result<CurrentUser, AuthError> {
        match self.mode {
            Mode::Local => Ok(CurrentUser::local_admin()),
            Mode::Server => {
                let token = crate::auth::token_from_headers(&parts.headers)
                    .ok_or(AuthError::Unauthorized)?;
                crate::auth::AuthService::new(self.db.clone())
                    .authenticate(&token)
                    .await
            }
        }
    }
}
