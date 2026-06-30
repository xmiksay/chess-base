//! HTTP surface for the Threats overlay (issue #123): a single read-only
//! endpoint returning the threatened-piece arrows for a position as JSON. Pure
//! and stateless — it only parses the FEN, so it needs no DB or engine, but it
//! sits behind the same [`CurrentUser`] gate as the rest of the API.

use axum::{
    extract::Query,
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use serde::Deserialize;

use crate::position::{CastlingMode, PositionError};
use crate::server::error::error_response;
use crate::server::identity::CurrentUser;
use crate::server::state::AppState;
use crate::threats::threats;

/// Threats route, mounted under the main API router.
pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/api/threats", get(get_threats))
        .with_state(state)
}

#[derive(Deserialize)]
struct ThreatsQuery {
    fen: String,
}

/// `GET /api/threats?fen=…` — red arrows for the side-to-move's hanging pieces.
async fn get_threats(
    _user: CurrentUser,
    Query(q): Query<ThreatsQuery>,
) -> Result<Response, ThreatsError> {
    let shapes = threats(&q.fen, CastlingMode::Standard)?;
    Ok((StatusCode::OK, Json(shapes)).into_response())
}

/// Wrapper so a bad FEN maps to `400` with a JSON error envelope (never a panic).
struct ThreatsError(PositionError);

impl From<PositionError> for ThreatsError {
    fn from(e: PositionError) -> Self {
        ThreatsError(e)
    }
}

impl IntoResponse for ThreatsError {
    fn into_response(self) -> Response {
        error_response(StatusCode::BAD_REQUEST, self.0.to_string())
    }
}
