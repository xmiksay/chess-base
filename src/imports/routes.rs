//! HTTP surface for game import: trigger a provider sync or upload a PGN into a
//! target database. Thin callers of [`ImportService`] that translate JSON ⇄ the
//! service; the write guard and provider dispatch live in the service.

use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::post,
    Json, Router,
};
use serde::Deserialize;
use serde_json::json;

use crate::imports::{ImportError, ImportService, ImportSource};
use crate::server::error::error_response;
use crate::server::identity::CurrentUser;
use crate::server::state::AppState;

/// Import routes, mounted under the main API router.
pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/api/import/pgn", post(import_pgn))
        .route("/api/import/sync", post(sync))
        .with_state(state)
}

#[derive(Deserialize)]
struct PgnBody {
    database_id: i32,
    /// PGN text — one or many games — to ingest into the target database.
    pgn: String,
}

#[derive(Deserialize)]
struct SyncBody {
    database_id: i32,
    /// Provider tag: `"lichess"` or `"chesscom"`.
    source: String,
    username: String,
    /// Optional personal token (Lichess; raises rate limits). Blank ⇒ absent.
    #[serde(default)]
    token: Option<String>,
}

/// Upload a PGN into a database (`POST /api/import/pgn`).
async fn import_pgn(
    State(state): State<AppState>,
    user: CurrentUser,
    Json(body): Json<PgnBody>,
) -> Result<Response, ImportError> {
    let summary = service(&state)
        .import_pgn(&user, body.database_id, &body.pgn)
        .await?;
    Ok((
        StatusCode::OK,
        Json(json!({ "imported": summary.imported })),
    )
        .into_response())
}

/// Trigger a Lichess / Chess.com sync into a database (`POST /api/import/sync`).
async fn sync(
    State(state): State<AppState>,
    user: CurrentUser,
    Json(body): Json<SyncBody>,
) -> Result<Response, ImportError> {
    let source = ImportSource::parse(&body.source).ok_or_else(|| {
        ImportError::InvalidInput(format!(
            "unknown source '{}' (expected lichess or chesscom)",
            body.source
        ))
    })?;
    let summary = service(&state)
        .sync(
            &user,
            body.database_id,
            source,
            &body.username,
            body.token.as_deref(),
        )
        .await?;
    Ok((
        StatusCode::OK,
        Json(json!({ "imported": summary.imported })),
    )
        .into_response())
}

fn service(state: &AppState) -> ImportService {
    ImportService::new(state.db.clone())
}

/// Map service failures onto HTTP status + a JSON error envelope. 5xx details are
/// internal; clients get a generic message (never a raw `DbErr`). A failed
/// sync/ingest is reported as a 400 with the collector/ingest message, which is
/// safe to surface and actionable for the user (bad username, malformed PGN, …).
impl IntoResponse for ImportError {
    fn into_response(self) -> Response {
        let status = match &self {
            ImportError::NotFound => StatusCode::NOT_FOUND,
            ImportError::Forbidden => StatusCode::FORBIDDEN,
            ImportError::InvalidInput(_) | ImportError::Failed(_) => StatusCode::BAD_REQUEST,
            ImportError::Db(_) => StatusCode::INTERNAL_SERVER_ERROR,
        };
        error_response(status, self.to_string())
    }
}
