//! HTTP surface for the position-explorer "Add line to study" action (#173):
//! `POST /api/studies/add-line`, a thin caller over [`StudyService::add_line`]
//! that grafts a flat SAN line (from the standard start) into a study, creating
//! it first when `study_id` is omitted — mirrors the new-vs-existing split of
//! `POST /api/studies/merge-games`.

use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::post,
    Json, Router,
};
use serde::Deserialize;

use crate::server::identity::CurrentUser;
use crate::server::state::AppState;
use crate::studies::routes::StudyView;
use crate::studies::{StudyError, StudyService};

/// Add-line route, merged into the main API router.
pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/api/studies/add-line", post(add_line))
        .with_state(state)
}

/// Body for `POST /api/studies/add-line`. `study_id` set ⇒ graft into that
/// existing study; omitted ⇒ create a new one from `database_id`/`name`
/// (both required in that case), filed into `folder_id`.
#[derive(Deserialize)]
struct AddLineBody {
    sans: Vec<String>,
    #[serde(default)]
    study_id: Option<i32>,
    #[serde(default)]
    database_id: Option<i32>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    folder_id: Option<i32>,
    /// Attached to the line's final node — e.g. the explorer's
    /// "N games, W/D/L" stat for that position.
    #[serde(default)]
    comment: Option<String>,
}

/// Graft a SAN line into a study (`POST /api/studies/add-line`), creating it
/// first when `study_id` is omitted. `201` on creation, `200` when grafted into
/// an existing study — the same status split as `POST /api/studies/merge-games`.
async fn add_line(
    State(state): State<AppState>,
    user: CurrentUser,
    Json(body): Json<AddLineBody>,
) -> Result<Response, StudyError> {
    let created = body.study_id.is_none();
    let model = StudyService::new(state.db.clone())
        .add_line(
            &user,
            &body.sans,
            body.study_id,
            body.database_id,
            body.name,
            body.folder_id,
            body.comment,
        )
        .await?;
    let status = if created {
        StatusCode::CREATED
    } else {
        StatusCode::OK
    };
    Ok((status, Json(StudyView::try_from(model)?)).into_response())
}
