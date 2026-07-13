//! HTTP surface for the standalone transposition-annotation pass (issue #174):
//! `POST /api/studies/{id}/mark-transpositions`, a thin caller over
//! [`StudyService::mark_transpositions`]. Split out of `routes.rs` (already over
//! the file-size cap), mirroring `danger_route.rs`'s own small merged router.

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::post,
    Json, Router,
};

use crate::server::identity::CurrentUser;
use crate::server::state::AppState;
use crate::studies::routes::StudyView;
use crate::studies::{StudyError, StudyService};

/// Transposition-marking route, merged into the main API router.
pub fn router(state: AppState) -> Router {
    Router::new()
        .route(
            "/api/studies/{id}/mark-transpositions",
            post(mark_transpositions),
        )
        .with_state(state)
}

/// Tag transposing lines in a study with a note pointing at the earlier
/// (canonical) node reaching the same position
/// (`POST /api/studies/{id}/mark-transpositions`, issue #174). Thin caller over
/// [`StudyService::mark_transpositions`]; returns the refreshed [`StudyView`] so
/// the editor re-renders the notes from one response.
async fn mark_transpositions(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<i32>,
) -> Result<Response, StudyError> {
    let model = StudyService::new(state.db.clone())
        .mark_transpositions(&user, id)
        .await?;
    Ok((StatusCode::OK, Json(StudyView::try_from(model)?)).into_response())
}
