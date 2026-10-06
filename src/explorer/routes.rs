//! HTTP surface for the Lichess Masters explorer (ADR-0053). Signed-in callers
//! only (the `CurrentUser` extractor 401s an anonymous server-mode request);
//! `503` when no `LICHESS_TOKEN` is configured, which `/api/health`'s `masters`
//! flag lets the SPA hide up front.

use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;

use super::{MastersClient, MastersError, MastersQuery};
use crate::imports::{routes::summary_body, ImportError, ImportService};
use crate::server::error::error_response;
use crate::server::identity::CurrentUser;
use crate::server::state::AppState;

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/api/explorer/masters", get(report))
        .route("/api/explorer/masters/import", post(import))
        .with_state(state)
}

#[derive(Deserialize)]
struct ImportBody {
    lichess_id: String,
    database_id: i32,
}

/// `GET /api/explorer/masters?fen=…&since=YYYY&until=YYYY`
async fn report(
    State(state): State<AppState>,
    _user: CurrentUser,
    Query(q): Query<MastersQuery>,
) -> Result<Response, ExplorerError> {
    let report = client(&state)?.report(&q).await?;
    Ok(Json(report).into_response())
}

/// `POST /api/explorer/masters/import` — fetch one masters game's PGN and ingest
/// it (deduped by source ref) into a database the caller may write.
async fn import(
    State(state): State<AppState>,
    user: CurrentUser,
    Json(body): Json<ImportBody>,
) -> Result<Response, ExplorerError> {
    let pgn = client(&state)?.game_pgn(&body.lichess_id).await?;
    let summary = ImportService::new(state.db.clone())
        .import_pgn(&user, body.database_id, &pgn)
        .await?;
    Ok((StatusCode::OK, Json(summary_body(&summary))).into_response())
}

fn client(state: &AppState) -> Result<&MastersClient, ExplorerError> {
    state.masters.as_deref().ok_or(ExplorerError::Disabled)
}

pub enum ExplorerError {
    Disabled,
    Masters(MastersError),
    Import(ImportError),
}

impl From<MastersError> for ExplorerError {
    fn from(e: MastersError) -> Self {
        Self::Masters(e)
    }
}

impl From<ImportError> for ExplorerError {
    fn from(e: ImportError) -> Self {
        Self::Import(e)
    }
}

impl IntoResponse for ExplorerError {
    fn into_response(self) -> Response {
        match self {
            // 503 bodies are masked by `error_response`; the status is the signal.
            Self::Disabled => {
                error_response(StatusCode::SERVICE_UNAVAILABLE, "masters explorer disabled")
            }
            Self::Masters(e) => {
                let status = match &e {
                    MastersError::BadRequest(_) => StatusCode::BAD_REQUEST,
                    MastersError::NotFound => StatusCode::NOT_FOUND,
                    MastersError::RateLimited => StatusCode::TOO_MANY_REQUESTS,
                    MastersError::Upstream(err) => {
                        tracing::warn!(error = %format!("{err:#}"), "masters explorer upstream failure");
                        StatusCode::BAD_GATEWAY
                    }
                };
                error_response(status, e.to_string())
            }
            Self::Import(e) => e.into_response(),
        }
    }
}
