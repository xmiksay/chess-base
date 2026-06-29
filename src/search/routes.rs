//! HTTP surface for position search (ADR-0003): "find games reaching this
//! position" and the opening tree of per-continuation statistics. Both stream
//! their rows as NDJSON (`application/x-ndjson`, one JSON object per line). Thin
//! callers of [`PositionSearchService`]; all scoping lives in the service.

use axum::{
    body::{Body, Bytes},
    extract::{Query, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use serde::{Deserialize, Serialize};

use crate::search::headers::{HeaderParams, HeaderQuery, HeaderSearchError, HeaderSearchService};
use crate::search::position::{PositionSearchService, SearchError};
use crate::server::error::error_response;
use crate::server::identity::CurrentUser;
use crate::server::state::AppState;

/// Search routes, mounted under the main API router.
pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/api/search/tree", get(tree))
        .route("/api/search/games", get(games))
        .route("/api/search/headers", get(headers))
        .with_state(state)
}

/// `?fen=<FEN>` query, shared by both endpoints. `limit` caps the games endpoint.
#[derive(Deserialize)]
struct SearchQuery {
    fen: String,
    #[serde(default)]
    limit: Option<u64>,
}

/// `GET /api/search/tree?fen=…` — per-continuation move statistics as NDJSON.
async fn tree(
    State(state): State<AppState>,
    user: CurrentUser,
    Query(q): Query<SearchQuery>,
) -> Result<Response, SearchError> {
    let stats = service(&state).opening_tree(&user, &q.fen).await?;
    ndjson(stats)
}

/// `GET /api/search/games?fen=…&limit=…` — games reaching the position, NDJSON.
async fn games(
    State(state): State<AppState>,
    user: CurrentUser,
    Query(q): Query<SearchQuery>,
) -> Result<Response, SearchError> {
    let hits = service(&state)
        .games_with_position(&user, &q.fen, q.limit)
        .await?;
    ndjson(hits)
}

/// `GET /api/search/headers?player=…&color=…&event=…&eco=…&date_from=…&date_to=…
/// &result=…&sort=…&dir=…&limit=…&cursor=…` — keyset-paginated header search. Unlike
/// the position endpoints this returns a single JSON page `{ games, next_cursor }`
/// so the opaque cursor travels with its rows.
async fn headers(
    State(state): State<AppState>,
    user: CurrentUser,
    Query(params): Query<HeaderParams>,
) -> Result<Response, HeaderSearchError> {
    let query = HeaderQuery::try_from(params)?;
    let page = HeaderSearchService::new(state.db.clone())
        .search(&user, &query)
        .await?;
    Ok((StatusCode::OK, Json(page)).into_response())
}

/// Build an NDJSON streaming response: each item is serialized to its own line
/// and emitted as a separate body chunk (`application/x-ndjson`).
fn ndjson<T: Serialize>(items: Vec<T>) -> Result<Response, SearchError> {
    let mut chunks: Vec<Result<Bytes, std::io::Error>> = Vec::with_capacity(items.len());
    for item in &items {
        let mut line = serde_json::to_string(item)?;
        line.push('\n');
        chunks.push(Ok(Bytes::from(line)));
    }
    let body = Body::from_stream(tokio_stream::iter(chunks));
    Ok((
        StatusCode::OK,
        [(header::CONTENT_TYPE, "application/x-ndjson")],
        body,
    )
        .into_response())
}

fn service(state: &AppState) -> PositionSearchService {
    PositionSearchService::new(state.db.clone())
}

/// Map service failures onto HTTP status + a JSON error envelope. 5xx details are
/// internal; clients get a generic message (never a raw `DbErr`).
impl IntoResponse for SearchError {
    fn into_response(self) -> Response {
        let status = match &self {
            SearchError::InvalidFen(_) => StatusCode::BAD_REQUEST,
            SearchError::Serialize(_) | SearchError::Db(_) => StatusCode::INTERNAL_SERVER_ERROR,
        };
        error_response(status, self.to_string())
    }
}

/// Map header-search failures onto HTTP status + a JSON error envelope. Bad
/// filters / cursors are client errors; DB / serialization faults stay internal.
impl IntoResponse for HeaderSearchError {
    fn into_response(self) -> Response {
        let status = match &self {
            HeaderSearchError::BadRequest(_) | HeaderSearchError::InvalidCursor => {
                StatusCode::BAD_REQUEST
            }
            HeaderSearchError::Serialize(_) | HeaderSearchError::Db(_) => {
                StatusCode::INTERNAL_SERVER_ERROR
            }
        };
        error_response(status, self.to_string())
    }
}
