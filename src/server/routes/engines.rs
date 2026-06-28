//! HTTP surface for the engine registry: list / add-edit / remove engines and
//! read or set the default. Thin callers of [`EngineRegistry`] that translate
//! JSON ⇄ [`EngineConfig`] and map [`RegistryError`] onto a status. Reads are
//! open to any authenticated caller; writes are admin-gated in the service.

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{delete, get},
    Json, Router,
};
use serde::Deserialize;
use serde_json::json;

use crate::engine::{EngineConfig, RegistryError};
use crate::server::{identity::CurrentUser, state::AppState};

/// Engine-registry routes, mounted under the main API router.
pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/api/engines", get(list).post(upsert))
        .route("/api/engines/default", get(get_default).put(set_default))
        .route("/api/engines/{name}", delete(remove))
        .with_state(state)
}

/// `GET /api/engines` — every registered engine.
async fn list(State(state): State<AppState>, _user: CurrentUser) -> Result<Response, ApiError> {
    let engines = state.engines().list().await?;
    Ok(Json(engines).into_response())
}

/// `POST /api/engines` — add or replace an engine (keyed by name). Admin-only.
async fn upsert(
    State(state): State<AppState>,
    user: CurrentUser,
    Json(config): Json<EngineConfig>,
) -> Result<Response, ApiError> {
    state.engines().upsert(&user, config).await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

/// `DELETE /api/engines/{name}` — remove an engine. Admin-only.
async fn remove(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(name): Path<String>,
) -> Result<Response, ApiError> {
    state.engines().remove(&user, &name).await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

/// `GET /api/engines/default` — the selected default name plus the engine the
/// resolution order actually settles on.
async fn get_default(
    State(state): State<AppState>,
    _user: CurrentUser,
) -> Result<Response, ApiError> {
    let registry = state.engines();
    let body = json!({
        "default": registry.default_name().await?,
        "resolved": registry.resolve_default().await?,
    });
    Ok(Json(body).into_response())
}

#[derive(Deserialize)]
struct DefaultSelection {
    name: String,
}

/// `PUT /api/engines/default` — point the default selector at a registered
/// engine. Admin-only.
async fn set_default(
    State(state): State<AppState>,
    user: CurrentUser,
    Json(body): Json<DefaultSelection>,
) -> Result<Response, ApiError> {
    state.engines().set_default(&user, &body.name).await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

/// Route-level error mapping registry failures onto HTTP statuses.
struct ApiError(RegistryError);

impl From<RegistryError> for ApiError {
    fn from(e: RegistryError) -> Self {
        ApiError(e)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, message) = match self.0 {
            RegistryError::NotFound(name) => {
                (StatusCode::NOT_FOUND, format!("engine '{name}' not found"))
            }
            RegistryError::Forbidden => (
                StatusCode::FORBIDDEN,
                "admin privileges required".to_string(),
            ),
            // Corrupt settings / DB errors are internal; clients get a generic message.
            RegistryError::Serde(_) | RegistryError::Db(_) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal error".to_string(),
            ),
        };
        (status, Json(json!({ "error": message }))).into_response()
    }
}
