//! HTTP surface for the annotated danger-map merge (issue #177, ADR-0032):
//! `POST /api/studies/{id}/merge-danger`, a thin caller over
//! [`StudyService::merge_danger`]. Split out of `routes.rs` (already over the
//! project's line cap) for the same reason `danger_route.rs` / `add_line_route.rs`
//! are their own files.

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::post,
    Json, Router,
};
use serde::{Deserialize, Serialize};

use crate::server::identity::CurrentUser;
use crate::server::state::AppState;
use crate::studies::merge_danger::MergeDangerOutcome;
use crate::studies::routes::StudyView;
use crate::studies::{StudyError, StudyService};
use crate::study_gen::DangerTree;

/// Merge-danger route, merged into the main API router.
pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/api/studies/{id}/merge-danger", post(merge_danger))
        .with_state(state)
}

/// Body for `POST /api/studies/{id}/merge-danger`: graft an engine-walked
/// [`DangerTree`] into the study's move tree as deduped variations. `at_node_id`
/// chooses the graft point (defaults to the root / start position).
#[derive(Deserialize)]
struct MergeDangerBody {
    tree: DangerTree,
    #[serde(default)]
    at_node_id: Option<usize>,
}

/// Response for `POST /api/studies/{id}/merge-danger`: the refreshed
/// [`StudyView`] plus what the graft actually added (issue #177), so the FE
/// "Extend this study" action can report "N new nodes, W Weapons, C Cautions"
/// instead of a silent success, and "no new lines" on an idempotent re-merge.
#[derive(Serialize)]
struct MergeDangerView {
    #[serde(flatten)]
    study: StudyView,
    added_nodes: usize,
    weapons: usize,
    cautions: usize,
}

impl TryFrom<MergeDangerOutcome> for MergeDangerView {
    type Error = StudyError;

    fn try_from(outcome: MergeDangerOutcome) -> Result<Self, Self::Error> {
        Ok(Self {
            added_nodes: outcome.added_nodes,
            weapons: outcome.weapons,
            cautions: outcome.cautions,
            study: StudyView::try_from(outcome.study)?,
        })
    }
}

/// Merge a danger tree into an existing study
/// (`POST /api/studies/{id}/merge-danger`). Thin caller over
/// [`StudyService::merge_danger`]; returns the refreshed [`StudyView`] plus the
/// graft's added-node/role counts (200) so the editor re-renders the grafted
/// variations from one response.
async fn merge_danger(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<i32>,
    Json(body): Json<MergeDangerBody>,
) -> Result<Response, StudyError> {
    let outcome = StudyService::new(state.db.clone())
        .merge_danger(&user, id, body.tree, body.at_node_id)
        .await?;
    Ok((StatusCode::OK, Json(MergeDangerView::try_from(outcome)?)).into_response())
}
