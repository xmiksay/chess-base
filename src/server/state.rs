//! Shared, cloneable runtime state injected into every handler.

use std::sync::Arc;

use axum::http::request::Parts;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use sea_orm::DatabaseConnection;

use crate::ai::agent::{AgentEngine, AgentProviderStore};
use crate::ai::llm::entanglement::StackLlmProvider;
use crate::ai::llm::LlmProvider;
use crate::engine::{EngineRegistry, EngineService};
use crate::server::config::Mode;
use crate::server::identity::{AuthError, CurrentUser};
use entanglement_provider::UserId;

#[derive(Clone)]
pub struct AppState {
    pub db: DatabaseConnection,
    pub mode: Mode,
    /// Pooled one-shot engine facade backing the batch `analyse` API and the MCP
    /// `engine_analyse` tool. Built at startup from the registry's resolved
    /// default; `None` ⇒ those paths are disabled.
    pub engine_service: Option<Arc<EngineService>>,
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

    /// The calling user's batch-LLM provider (#198, step 6): their default
    /// `llm_providers` row (falling back house-wards, see
    /// `AgentProviderStore::default_for`) resolved through the agent engine's
    /// model resolver. `None` ⇒ no engine running, nothing configured for this
    /// user, or the default row failed to resolve (logged) — callers surface
    /// their existing 503 "no LLM provider" behavior.
    pub fn llm_for(&self, user: &CurrentUser) -> Option<Arc<dyn LlmProvider>> {
        let engine = self.agent()?;
        let uid = UserId::new(user.id.clone());
        let (provider, model) = engine.providers.default_for(&uid)?;
        match StackLlmProvider::resolve(&engine.resolver, Some(&uid), &provider, &model) {
            Ok(resolved) => Some(Arc::new(resolved)),
            Err(err) => {
                tracing::warn!(user = %user.id, provider, model, error = %err,
                    "default LLM provider failed to resolve");
                None
            }
        }
    }

    /// As [`Self::llm_for`], but honoring an explicit per-request `(provider,
    /// model)` choice (issue #214). No `provider` ⇒ the caller's default row
    /// (any `model` override is applied downstream, as today); a named
    /// `provider` resolves that row for this user through the same resolver
    /// seam the agent engine uses, and requires `model`. A choice the resolver
    /// rejects (unknown name, unusable row) is the caller's error, distinct
    /// from "nothing configured".
    pub fn llm_for_choice(
        &self,
        user: &CurrentUser,
        provider: Option<&str>,
        model: Option<&str>,
    ) -> Result<Arc<dyn LlmProvider>, LlmChoiceError> {
        let Some(name) = provider else {
            return self.llm_for(user).ok_or(LlmChoiceError::Unconfigured);
        };
        let Some(model) = model else {
            return Err(LlmChoiceError::Invalid(
                "`provider` requires `model` to be set".to_string(),
            ));
        };
        let engine = self.agent().ok_or(LlmChoiceError::Unconfigured)?;
        let uid = UserId::new(user.id.clone());
        let resolved = StackLlmProvider::resolve(&engine.resolver, Some(&uid), name, model)
            .map_err(|e| LlmChoiceError::Invalid(e.to_string()))?;
        Ok(Arc::new(resolved))
    }
}

/// Why [`AppState::llm_for_choice`] failed — the routes map these onto HTTP
/// statuses via the [`IntoResponse`] impl below.
#[derive(Debug)]
pub enum LlmChoiceError {
    /// No provider surface at all (no agent engine / nothing configured) — the
    /// routes' long-standing 503.
    Unconfigured,
    /// The caller's explicit provider/model choice is invalid — a 400 carrying
    /// the resolver's message (never keys or internals).
    Invalid(String),
}

impl IntoResponse for LlmChoiceError {
    fn into_response(self) -> Response {
        match self {
            LlmChoiceError::Unconfigured => (
                StatusCode::SERVICE_UNAVAILABLE,
                "No language model configured: add an LLM provider to enable study generation.",
            )
                .into_response(),
            LlmChoiceError::Invalid(msg) => {
                crate::server::error::error_response(StatusCode::BAD_REQUEST, msg)
            }
        }
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

#[cfg(test)]
mod tests;
