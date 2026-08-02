//! HTTP surface for the per-user LLM provider registry (issue #20, per-user
//! since #198). API keys are write-only — stored server-side, never returned
//! to the SPA (responses carry a `has_key` flag instead).
//!
//! Every authenticated user manages their own rows; global rows (visible to
//! everyone) stay admin-only via `is_global`. After a successful mutation the
//! agent provider store's cache is invalidated so the next model resolution
//! sees the change.

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use serde::Deserialize;

use crate::ai::providers::{ProviderInput, ProviderService, ProviderStoreError};
use crate::server::error::error_response;
use crate::server::identity::CurrentUser;
use crate::server::state::AppState;

/// Provider-registry routes, mounted under the main API router. The paths keep
/// the pre-#198 `/api/assistant/providers` prefix so the SPA keeps working.
pub fn router(state: AppState) -> Router {
    Router::new()
        .route(
            "/api/assistant/providers",
            get(list_providers).post(upsert_provider),
        )
        .route(
            "/api/assistant/providers/{id}",
            axum::routing::delete(delete_provider),
        )
        .with_state(state)
}

/// Rebuild the agent store's per-user catalogs after a provider mutation.
/// Best-effort: a failed refresh is logged, the mutation already succeeded.
async fn invalidate_store(state: &AppState, owner: Option<&str>) {
    if let Some(store) = &state.provider_store {
        if let Err(e) = store.invalidate(owner).await {
            tracing::warn!(error = %format!("{e:#}"), "provider store refresh failed");
        }
    }
}

async fn list_providers(
    State(state): State<AppState>,
    user: CurrentUser,
) -> Result<Response, ProviderStoreError> {
    let providers = ProviderService::new(state.db.clone()).list(&user).await?;
    Ok((StatusCode::OK, Json(providers)).into_response())
}

#[derive(Deserialize)]
struct ProviderBody {
    name: String,
    #[serde(default = "default_wire")]
    wire: String,
    model: String,
    #[serde(default)]
    base_url: Option<String>,
    /// Secret key, write-only — omitted ⇒ keep the stored key.
    #[serde(default)]
    api_key: Option<String>,
    #[serde(default)]
    is_default: bool,
    /// Write a global row (admin only).
    #[serde(default)]
    is_global: bool,
}

fn default_wire() -> String {
    "anthropic".to_string()
}

async fn upsert_provider(
    State(state): State<AppState>,
    user: CurrentUser,
    Json(body): Json<ProviderBody>,
) -> Result<Response, ProviderStoreError> {
    let owner = (!body.is_global).then(|| user.id.clone());
    let info = ProviderService::new(state.db.clone())
        .upsert(
            &user,
            ProviderInput {
                name: body.name,
                wire: body.wire,
                model: body.model,
                base_url: body.base_url,
                api_key: body.api_key,
                is_default: body.is_default,
                is_global: body.is_global,
            },
        )
        .await?;
    invalidate_store(&state, owner.as_deref()).await;
    Ok((StatusCode::OK, Json(info)).into_response())
}

async fn delete_provider(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<i32>,
) -> Result<Response, ProviderStoreError> {
    ProviderService::new(state.db.clone())
        .delete(&user, id)
        .await?;
    invalidate_store(&state, Some(&user.id)).await;
    Ok(StatusCode::NO_CONTENT.into_response())
}

// --- Error mapping (transport edge) --------------------------------------

impl IntoResponse for ProviderStoreError {
    fn into_response(self) -> Response {
        let (status, message) = match &self {
            ProviderStoreError::Forbidden => (StatusCode::FORBIDDEN, "not permitted".to_string()),
            ProviderStoreError::NotFound => {
                (StatusCode::NOT_FOUND, "provider not found".to_string())
            }
            ProviderStoreError::Db(_) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal error".to_string(),
            ),
        };
        error_response(status, message)
    }
}
