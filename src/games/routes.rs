//! HTTP surface for game listing (issue #68): a keyset-paginated list of the
//! games in a database, and a single-game fetch (with PGN) for board playback.
//! Thin callers of [`GameService`]; all scoping lives in the service.

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use serde::Deserialize;

use crate::games::{GameError, GameService};
use crate::server::error::error_response;
use crate::server::identity::CurrentUser;
use crate::server::state::AppState;

/// Game routes, mounted under the main API router.
pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/api/games", get(list))
        .route("/api/games/{id}", get(get_one))
        .with_state(state)
}

/// `?database_id=<id>&after=<id>&limit=<n>` for the list endpoint. `after` is the
/// keyset cursor (last id of the previous page); `limit` is clamped by the service.
#[derive(Deserialize)]
struct ListQuery {
    database_id: i32,
    #[serde(default)]
    after: Option<i32>,
    #[serde(default)]
    limit: Option<u64>,
}

/// `GET /api/games?database_id=…&after=…&limit=…` — one keyset page of games.
async fn list(
    State(state): State<AppState>,
    user: CurrentUser,
    Query(q): Query<ListQuery>,
) -> Result<Response, GameError> {
    let page = service(&state)
        .list(&user, q.database_id, q.after, q.limit)
        .await?;
    Ok((StatusCode::OK, Json(page)).into_response())
}

/// `GET /api/games/{id}` — a single game with its PGN movetext.
async fn get_one(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<i32>,
) -> Result<Response, GameError> {
    let game = service(&state).get(&user, id).await?;
    Ok((StatusCode::OK, Json(game)).into_response())
}

fn service(state: &AppState) -> GameService {
    GameService::new(state.db.clone())
}

/// Map service failures onto HTTP status + a JSON error envelope. 5xx details are
/// internal; clients get a generic message (never a raw `DbErr`).
impl IntoResponse for GameError {
    fn into_response(self) -> Response {
        let status = match &self {
            GameError::NotFound => StatusCode::NOT_FOUND,
            GameError::Db(_) => StatusCode::INTERNAL_SERVER_ERROR,
        };
        error_response(status, self.to_string())
    }
}
