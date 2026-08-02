//! Shared, cloneable runtime state injected into every handler.

use std::sync::Arc;

use axum::http::request::Parts;
use sea_orm::DatabaseConnection;

use crate::ai::agent::{AgentEngine, AgentProviderStore};
use crate::ai::llm::LlmProvider;
use crate::engine::{EngineRegistry, EngineService};
use crate::server::config::Mode;
use crate::server::identity::{AuthError, CurrentUser};

#[derive(Clone)]
pub struct AppState {
    pub db: DatabaseConnection,
    pub mode: Mode,
    /// Pooled one-shot engine facade backing the batch `analyse` API and the MCP
    /// `engine_analyse` tool. Built at startup from the registry's resolved
    /// default; `None` ⇒ those paths are disabled.
    pub engine_service: Option<Arc<EngineService>>,
    /// LLM provider backing AI-assisted study generation (#115). Always `None`
    /// until the entanglement agent engine re-wires provider resolution (#198);
    /// `None` ⇒ the `generate_study` paths are disabled.
    pub llm_provider: Option<Arc<dyn LlmProvider>>,
    /// Per-user provider store for the embedded entanglement engine (#198).
    /// Built at startup (it only needs the DB); provider CRUD invalidates it.
    /// `None` in tests that don't exercise the agent.
    pub provider_store: Option<Arc<AgentProviderStore>>,
    /// The embedded agent engine (#198, step 4). A `OnceLock` because the
    /// engine needs a fully-built `AppState` (its tool bridge closes over it),
    /// so `serve` sets it right after construction; empty ⇒ the engine failed
    /// to start (or a test fixture) and the assistant is disabled.
    pub agent: Arc<std::sync::OnceLock<Arc<AgentEngine>>>,
}

impl AppState {
    /// The persisted engine registry over this state's database connection. The
    /// analysis WebSocket and the engine routes resolve the engine through it,
    /// so engine selection is never duplicated on `AppState`.
    pub fn engines(&self) -> EngineRegistry {
        EngineRegistry::new(self.db.clone())
    }

    /// The running agent engine, if it started.
    pub fn agent(&self) -> Option<&Arc<AgentEngine>> {
        self.agent.get()
    }
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
