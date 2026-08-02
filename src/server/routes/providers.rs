//! HTTP surface for the admin-managed LLM provider registry (issue #20). API
//! keys are write-only — stored server-side, never returned to the SPA.
//!
//! The old assistant session/message routes that shared this file were removed
//! with the hand-rolled assistant (#198); the entanglement-backed agent surface
//! lands in later steps.

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

async fn list_providers(
    State(state): State<AppState>,
    user: CurrentUser,
) -> Result<Response, ProviderStoreError> {
    crate::server::identity::assert_admin(&user).map_err(|_| ProviderStoreError::Forbidden)?;
    let providers = ProviderService::new(state.db.clone()).list().await?;
    Ok((StatusCode::OK, Json(providers)).into_response())
}

#[derive(Deserialize)]
struct ProviderBody {
    name: String,
    model: String,
    /// Secret key, write-only — stored server-side, never returned.
    api_key: String,
    #[serde(default)]
    is_default: bool,
}

async fn upsert_provider(
    State(state): State<AppState>,
    user: CurrentUser,
    Json(body): Json<ProviderBody>,
) -> Result<Response, ProviderStoreError> {
    let info = ProviderService::new(state.db.clone())
        .upsert(
            &user,
            ProviderInput {
                name: body.name,
                model: body.model,
                api_key: body.api_key,
                is_default: body.is_default,
            },
        )
        .await?;
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
    Ok(StatusCode::NO_CONTENT.into_response())
}

// --- Error mapping (transport edge) --------------------------------------

impl IntoResponse for ProviderStoreError {
    fn into_response(self) -> Response {
        let (status, message) = match &self {
            ProviderStoreError::Forbidden => (
                StatusCode::FORBIDDEN,
                "admin privileges required".to_string(),
            ),
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
