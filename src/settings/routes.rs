//! HTTP surface for per-user settings: read the caller's settings and replace
//! them. Thin callers of [`SettingsService`] that translate JSON ⇄ [`UserSettings`]
//! and map [`SettingsError`] onto a status; all validation lives in the service
//! (so MCP can reuse it).

use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use serde_json::json;

use crate::server::identity::CurrentUser;
use crate::server::state::AppState;
use crate::settings::{SettingsError, SettingsService, UserSettings};

/// Settings routes, mounted under the main API router.
pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/api/settings", get(get_settings).put(put_settings))
        .with_state(state)
}

/// `GET /api/settings` — the caller's stored settings (all-default if unset).
async fn get_settings(
    State(state): State<AppState>,
    user: CurrentUser,
) -> Result<Response, SettingsError> {
    let settings = service(&state).get(&user).await?;
    Ok((StatusCode::OK, Json(settings)).into_response())
}

/// `PUT /api/settings` — replace the caller's settings wholesale.
async fn put_settings(
    State(state): State<AppState>,
    user: CurrentUser,
    Json(body): Json<UserSettings>,
) -> Result<Response, SettingsError> {
    let saved = service(&state).set(&user, body).await?;
    Ok((StatusCode::OK, Json(saved)).into_response())
}

fn service(state: &AppState) -> SettingsService {
    SettingsService::new(state.db.clone())
}

/// Map service failures onto HTTP status + a JSON error envelope. 5xx details are
/// internal; clients get a generic message (never a raw `DbErr`).
impl IntoResponse for SettingsError {
    fn into_response(self) -> Response {
        let (status, message) = match &self {
            SettingsError::InvalidInput(_) => (StatusCode::BAD_REQUEST, self.to_string()),
            SettingsError::Serde(_) | SettingsError::Db(_) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal error".to_string(),
            ),
        };
        (status, Json(json!({ "error": message }))).into_response()
    }
}
