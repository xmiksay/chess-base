//! Shared, cloneable runtime state injected into every handler.

use std::sync::Arc;

use axum::http::request::Parts;
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

    /// Resolve the caller of a public-readable route (issue #211, ADR-0045),
    /// backing the [`PublicUser`] extractor. Semantics mirror the MCP tier's
    /// `authenticate_mcp` (ADR-0043): local mode is the implicit admin; a
    /// server-mode request with **no** credential resolves to
    /// [`CurrentUser::anonymous`] instead of `401`; a credential that is
    /// present goes through the normal [`AuthService`] resolution, so an
    /// invalid/expired token still `401`s (it is a broken credential, not an
    /// anonymous visitor).
    ///
    /// [`PublicUser`]: crate::server::identity::PublicUser
    /// [`AuthService`]: crate::auth::AuthService
    pub async fn resolve_public_user(&self, parts: &Parts) -> Result<CurrentUser, AuthError> {
        match self.mode {
            Mode::Local => Ok(CurrentUser::local_admin()),
            Mode::Server => match crate::auth::token_from_headers(&parts.headers) {
                Some(token) => {
                    crate::auth::AuthService::new(self.db.clone())
                        .authenticate(&token)
                        .await
                }
                None => Ok(CurrentUser::anonymous()),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{connect, DbConfig};
    use axum::http::Request;

    async fn state(mode: Mode) -> AppState {
        AppState {
            db: connect(&DbConfig::in_memory()).await.unwrap(),
            mode,
            engine_service: None,
            provider_store: None,
            agent: Default::default(),
        }
    }

    fn parts(auth: Option<&str>) -> Parts {
        let mut builder = Request::builder().uri("/");
        if let Some(value) = auth {
            builder = builder.header("authorization", value);
        }
        builder.body(()).unwrap().into_parts().0
    }

    #[tokio::test]
    async fn public_user_in_local_mode_is_the_implicit_admin() {
        let user = state(Mode::Local)
            .await
            .resolve_public_user(&parts(None))
            .await
            .unwrap();
        assert_eq!(user, CurrentUser::local_admin());
    }

    #[tokio::test]
    async fn public_user_without_credentials_is_anonymous_in_server_mode() {
        let user = state(Mode::Server)
            .await
            .resolve_public_user(&parts(None))
            .await
            .unwrap();
        assert_eq!(user, CurrentUser::anonymous());
    }

    #[tokio::test]
    async fn public_user_with_an_invalid_credential_is_still_unauthorized() {
        let err = state(Mode::Server)
            .await
            .resolve_public_user(&parts(Some("Bearer bogus")))
            .await
            .unwrap_err();
        assert_eq!(err, AuthError::Unauthorized);
    }
}
