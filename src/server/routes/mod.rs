//! HTTP routing: a small JSON API plus SPA static serving from embedded assets.

use axum::{
    body::Body,
    http::{header, StatusCode, Uri},
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use serde_json::json;

use crate::server::{embed::Assets, engine_ws, identity::CurrentUser, state::AppState};

mod mcp;

/// Build the application router.
pub fn router(state: AppState) -> Router {
    let api = Router::new()
        .route("/api/health", get(health))
        .route("/api/whoami", get(whoami))
        .route("/api/engine/analyse", get(engine_ws::analyse))
        .fallback(static_handler)
        .with_state(state.clone());

    // The MCP endpoint carries its own state (app + tool registry), so it is
    // resolved independently and merged in.
    api.merge(mcp::router(state))
}

/// Report the resolved caller: the implicit admin in local mode, the
/// authenticated user in server mode (once #14 wires auth). Exercises the
/// [`CurrentUser`] extractor and lets the SPA gate admin-only UI.
async fn whoami(user: CurrentUser) -> impl IntoResponse {
    Json(json!({ "id": user.id, "is_admin": user.is_admin }))
}

async fn health(axum::extract::State(state): axum::extract::State<AppState>) -> impl IntoResponse {
    let mode = match state.mode {
        crate::server::Mode::Local => "local",
        crate::server::Mode::Server => "server",
    };
    Json(json!({
        "status": "ok",
        "name": "chess-base",
        "version": env!("CARGO_PKG_VERSION"),
        "mode": mode,
    }))
}

/// Serve an embedded asset, falling back to `index.html` for SPA routes.
async fn static_handler(uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    let path = if path.is_empty() { "index.html" } else { path };

    if let Some(file) = Assets::get(path) {
        return (
            [(header::CONTENT_TYPE, file.metadata.mimetype())],
            file.data.into_owned(),
        )
            .into_response();
    }

    // Unknown path → serve the SPA shell so client-side routing works.
    match Assets::get("index.html") {
        Some(file) => (
            [(header::CONTENT_TYPE, "text/html")],
            file.data.into_owned(),
        )
            .into_response(),
        None => (StatusCode::NOT_FOUND, Body::empty()).into_response(),
    }
}
