//! HTTP surface for account management (ADR-0055): the caller changing their
//! own password, and the admin user list + password reset. Thin callers over
//! [`AuthService`]; server mode only, like the rest of `/api/auth`. Not
//! mirrored as MCP tools (see `routes/mcp/symmetry.rs` carve-outs).

use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, put},
    Json, Router,
};
use serde::Deserialize;

use super::routes::{require_server_mode, AuthApiError};
use crate::auth::{token_from_headers, AuthService, AuthServiceError};
use crate::server::identity::CurrentUser;
use crate::server::state::AppState;

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/api/auth/password", put(change_password))
        .route("/api/admin/users", get(list_users))
        .route("/api/admin/users/{id}/password", put(reset_password))
        .with_state(state)
}

#[derive(Deserialize)]
struct ChangePasswordBody {
    current_password: String,
    new_password: String,
}

#[derive(Deserialize)]
struct ResetPasswordBody {
    new_password: String,
}

async fn change_password(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<ChangePasswordBody>,
) -> Result<Response, AuthApiError> {
    require_server_mode(&state)?;
    // The raw token, not the `CurrentUser` extractor: the service must know
    // *which* session to keep, and that it is a session at all.
    let token = token_from_headers(&headers).ok_or(AuthServiceError::NotASession)?;
    AuthService::new(state.db.clone())
        .change_password(&token, &body.current_password, &body.new_password)
        .await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

async fn list_users(
    State(state): State<AppState>,
    user: CurrentUser,
) -> Result<Response, AuthApiError> {
    require_server_mode(&state)?;
    let users = AuthService::new(state.db.clone()).list_users(&user).await?;
    Ok(Json(users).into_response())
}

async fn reset_password(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<String>,
    Json(body): Json<ResetPasswordBody>,
) -> Result<Response, AuthApiError> {
    require_server_mode(&state)?;
    AuthService::new(state.db.clone())
        .admin_reset_password(&user, &id, &body.new_password)
        .await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}
