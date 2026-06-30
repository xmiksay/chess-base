//! HTTP surface for folders (issue #164): the directory tree that organizes
//! studies. Thin callers of [`FolderService`] that translate JSON ⇄ models; all
//! ownership/admin gating and the cycle/cascade rules live in the service.

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, patch},
    Json, Router,
};
use serde::{Deserialize, Serialize};

use crate::db::entities::folders;
use crate::folders::{FolderError, FolderService};
use crate::server::error::error_response;
use crate::server::identity::CurrentUser;
use crate::server::state::AppState;

/// Folder routes, mounted under the main API router.
pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/api/folders", get(list).post(create))
        .route("/api/folders/{id}", patch(update).delete(delete))
        .with_state(state)
}

/// A folder row for the SPA's tree (it assembles the hierarchy from `parent_id`).
#[derive(Serialize)]
struct FolderSummary {
    id: i32,
    owner_id: Option<String>,
    parent_id: Option<i32>,
    name: String,
    /// Convenience flag for the SPA: `owner_id IS NULL`.
    global: bool,
}

impl From<folders::Model> for FolderSummary {
    fn from(m: folders::Model) -> Self {
        Self {
            global: m.owner_id.is_none(),
            id: m.id,
            owner_id: m.owner_id,
            parent_id: m.parent_id,
            name: m.name,
        }
    }
}

#[derive(Deserialize)]
struct CreateBody {
    name: String,
    #[serde(default)]
    parent_id: Option<i32>,
    /// Create a global (admin-owned) folder; requires admin. Defaults to false.
    #[serde(default)]
    global: bool,
}

/// Body for `PATCH /api/folders/{id}`: rename (when `name` is present) and/or move
/// (when `reparent` is true, to `parent_id` — `null` = root). The explicit
/// `reparent` flag distinguishes "move to root" from "leave the parent alone".
#[derive(Deserialize)]
struct PatchBody {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    reparent: bool,
    #[serde(default)]
    parent_id: Option<i32>,
}

async fn list(State(state): State<AppState>, user: CurrentUser) -> Result<Response, FolderError> {
    let rows = service(&state).list(&user).await?;
    let views: Vec<FolderSummary> = rows.into_iter().map(FolderSummary::from).collect();
    Ok((StatusCode::OK, Json(views)).into_response())
}

async fn create(
    State(state): State<AppState>,
    user: CurrentUser,
    Json(body): Json<CreateBody>,
) -> Result<Response, FolderError> {
    let model = service(&state)
        .create(&user, body.parent_id, body.name, body.global)
        .await?;
    Ok((StatusCode::CREATED, Json(FolderSummary::from(model))).into_response())
}

async fn update(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<i32>,
    Json(body): Json<PatchBody>,
) -> Result<Response, FolderError> {
    let svc = service(&state);
    let mut model = None;
    if let Some(name) = body.name {
        model = Some(svc.rename(&user, id, name).await?);
    }
    if body.reparent {
        model = Some(svc.reparent(&user, id, body.parent_id).await?);
    }
    // Neither a rename nor a move requested: return the folder unchanged.
    let model = match model {
        Some(m) => m,
        None => svc
            .list(&user)
            .await?
            .into_iter()
            .find(|f| f.id == id)
            .ok_or(FolderError::NotFound)?,
    };
    Ok((StatusCode::OK, Json(FolderSummary::from(model))).into_response())
}

async fn delete(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<i32>,
) -> Result<Response, FolderError> {
    service(&state).delete(&user, id).await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

fn service(state: &AppState) -> FolderService {
    FolderService::new(state.db.clone())
}

/// Map service failures onto HTTP status + a JSON error envelope. 5xx details are
/// internal; clients get a generic message (never a raw `DbErr`).
impl IntoResponse for FolderError {
    fn into_response(self) -> Response {
        let status = match &self {
            FolderError::NotFound => StatusCode::NOT_FOUND,
            FolderError::Forbidden => StatusCode::FORBIDDEN,
            FolderError::Cycle => StatusCode::BAD_REQUEST,
            FolderError::Duplicate => StatusCode::CONFLICT,
            FolderError::Db(_) => StatusCode::INTERNAL_SERVER_ERROR,
        };
        error_response(status, self.to_string())
    }
}
