//! HTTP surface for the embedded Claude study assistant (issue #20, Direction B).
//!
//! Thin callers of [`AssistantService`] (the agent loop) and [`AssistantStore`]
//! (session persistence): create/list/read/delete chat sessions, post a message
//! to run the loop, and approve/deny the mutating tool calls it pauses on. The
//! admin-only `providers` sub-surface manages the `llm_providers` registry — API
//! keys are write-only and never returned.

use std::collections::HashMap;
use std::sync::Arc;

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;

use crate::ai::assistant::{
    build_view, AssistantError, AssistantService, AssistantStore, SessionSummary,
};
use crate::ai::providers::{ProviderInput, ProviderService, ProviderStoreError};
use crate::server::error::error_response;
use crate::server::identity::CurrentUser;
use crate::server::routes::mcp::default_registry;
use crate::server::state::AppState;

/// Assistant routes, mounted under the main API router.
pub fn router(state: AppState) -> Router {
    Router::new()
        .route(
            "/api/assistant/sessions",
            get(list_sessions).post(create_session),
        )
        .route(
            "/api/assistant/sessions/{id}",
            get(get_session).delete(delete_session),
        )
        .route("/api/assistant/sessions/{id}/messages", post(post_message))
        .route("/api/assistant/sessions/{id}/respond", post(respond))
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

fn store(state: &AppState) -> AssistantStore {
    AssistantStore::new(state.db.clone())
}

/// Build the loop service when an LLM provider is configured (the assistant needs
/// one; reads/deletes don't and skip this). `None` ⇒ the caller returns a `503`.
fn service(state: &AppState) -> Option<AssistantService> {
    let provider = state.llm_provider.clone()?;
    Some(AssistantService::new(
        state.clone(),
        provider,
        Arc::new(default_registry()),
    ))
}

/// The `503` returned when the assistant is used without an LLM provider.
fn no_provider() -> Response {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        "No language model configured: set ANTHROPIC_API_KEY or add an LLM provider.",
    )
        .into_response()
}

#[derive(Deserialize)]
struct CreateBody {
    #[serde(default)]
    title: Option<String>,
    /// Override the provider's default model for this session.
    #[serde(default)]
    model: Option<String>,
}

async fn create_session(
    State(state): State<AppState>,
    user: CurrentUser,
    Json(body): Json<CreateBody>,
) -> Result<Response, Response> {
    let view = service(&state)
        .ok_or_else(no_provider)?
        .create_session(&user, body.title, body.model)
        .await
        .map_err(IntoResponse::into_response)?;
    Ok((StatusCode::CREATED, Json(view)).into_response())
}

async fn list_sessions(
    State(state): State<AppState>,
    user: CurrentUser,
) -> Result<Response, AssistantError> {
    let rows = store(&state).list(&user).await?;
    let views: Vec<SessionSummary> = rows.into_iter().map(SessionSummary::from).collect();
    Ok((StatusCode::OK, Json(views)).into_response())
}

async fn get_session(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<i32>,
) -> Result<Response, AssistantError> {
    let store = store(&state);
    let session = store.get_owned(&user, id).await?;
    let messages = store.load_messages(id).await?;
    Ok((StatusCode::OK, Json(build_view(&session, &messages))).into_response())
}

async fn delete_session(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<i32>,
) -> Result<Response, AssistantError> {
    store(&state).delete(&user, id).await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

#[derive(Deserialize)]
struct MessageBody {
    text: String,
}

async fn post_message(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<i32>,
    Json(body): Json<MessageBody>,
) -> Result<Response, Response> {
    let view = service(&state)
        .ok_or_else(no_provider)?
        .post_message(&user, id, &body.text)
        .await
        .map_err(IntoResponse::into_response)?;
    Ok((StatusCode::OK, Json(view)).into_response())
}

#[derive(Deserialize)]
struct RespondBody {
    /// Per-call decision keyed by tool-call id: `true` approves, `false` denies.
    /// A gated call missing from the map defaults to denied.
    decisions: HashMap<String, bool>,
}

async fn respond(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<i32>,
    Json(body): Json<RespondBody>,
) -> Result<Response, Response> {
    let view = service(&state)
        .ok_or_else(no_provider)?
        .respond(&user, id, body.decisions)
        .await
        .map_err(IntoResponse::into_response)?;
    Ok((StatusCode::OK, Json(view)).into_response())
}

// --- Provider registry (admin) -------------------------------------------

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

impl IntoResponse for AssistantError {
    fn into_response(self) -> Response {
        let (status, message) = match &self {
            AssistantError::NotFound => (StatusCode::NOT_FOUND, "session not found".to_string()),
            AssistantError::Conflict(msg) => (StatusCode::CONFLICT, msg.clone()),
            // The provider failed (network / API / rate limit): a gateway error,
            // with a generic message — never the raw provider detail.
            AssistantError::Provider(_) => (
                StatusCode::BAD_GATEWAY,
                "the language model request failed".to_string(),
            ),
            AssistantError::Corrupt(_) | AssistantError::Db(_) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal error".to_string(),
            ),
        };
        error_response(status, message)
    }
}

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
