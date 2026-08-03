//! Admin HTTP surface for minting/listing/revoking service tokens (ADR-0044,
//! issue #193). Mirrors [`super::providers`]'s structure: thin callers over
//! [`ServiceTokenService`], which does the actual admin gate + scope
//! validation.

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use serde::Deserialize;

use crate::server::error::error_response;
use crate::server::identity::CurrentUser;
use crate::server::state::AppState;
use crate::service_tokens::{ServiceTokenError, ServiceTokenService};

pub fn router(state: AppState) -> Router {
    Router::new()
        .route(
            "/api/admin/service-tokens",
            get(list_tokens).post(create_token),
        )
        .route(
            "/api/admin/service-tokens/{id}",
            axum::routing::delete(revoke_token),
        )
        .with_state(state)
}

#[derive(Deserialize)]
struct CreateTokenBody {
    owner_id: String,
    label: String,
    scope: String,
    #[serde(default)]
    is_admin: bool,
    #[serde(default)]
    expires_in_days: Option<i64>,
}

async fn create_token(
    State(state): State<AppState>,
    user: CurrentUser,
    Json(body): Json<CreateTokenBody>,
) -> Result<Response, ServiceTokenError> {
    let minted = ServiceTokenService::new(state.db.clone())
        .create(
            &user,
            &body.owner_id,
            &body.label,
            &body.scope,
            body.is_admin,
            body.expires_in_days,
        )
        .await?;
    Ok((StatusCode::CREATED, Json(minted)).into_response())
}

async fn list_tokens(
    State(state): State<AppState>,
    user: CurrentUser,
) -> Result<Response, ServiceTokenError> {
    let tokens = ServiceTokenService::new(state.db.clone())
        .list(&user)
        .await?;
    Ok((StatusCode::OK, Json(tokens)).into_response())
}

async fn revoke_token(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<String>,
) -> Result<Response, ServiceTokenError> {
    ServiceTokenService::new(state.db.clone())
        .revoke(&user, &id)
        .await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

// --- Error mapping (transport edge) --------------------------------------

impl IntoResponse for ServiceTokenError {
    fn into_response(self) -> Response {
        let (status, message) = match &self {
            ServiceTokenError::Forbidden => (StatusCode::FORBIDDEN, "not permitted".to_string()),
            ServiceTokenError::NotFound => {
                (StatusCode::NOT_FOUND, "service token not found".to_string())
            }
            ServiceTokenError::InvalidInput(msg) => (StatusCode::BAD_REQUEST, msg.to_string()),
            ServiceTokenError::Db(_) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal error".to_string(),
            ),
        };
        error_response(status, message)
    }
}
