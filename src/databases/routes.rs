//! HTTP surface for database (collection) CRUD: list / create / get / rename /
//! delete. Thin callers of [`DatabaseService`] that translate JSON ⇄ models; all
//! ownership and admin gating lives in the service (so MCP reuses it).

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::databases::{DatabaseError, DatabaseService};
use crate::db::entities::databases;
use crate::server::identity::CurrentUser;
use crate::server::state::AppState;

/// Database routes, mounted under the main API router.
pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/api/databases", get(list).post(create))
        .route(
            "/api/databases/{id}",
            get(get_one).patch(rename).delete(delete),
        )
        .with_state(state)
}

/// Serializable view of a database row returned to clients.
#[derive(Serialize)]
struct DatabaseView {
    id: i32,
    owner_id: Option<String>,
    name: String,
    kind: String,
    index_depth: Option<i32>,
    /// Convenience flag for the SPA: `owner_id IS NULL`.
    global: bool,
}

impl From<databases::Model> for DatabaseView {
    fn from(m: databases::Model) -> Self {
        Self {
            global: m.owner_id.is_none(),
            id: m.id,
            owner_id: m.owner_id,
            name: m.name,
            kind: m.kind,
            index_depth: m.index_depth,
        }
    }
}

#[derive(Deserialize)]
struct CreateBody {
    name: String,
    kind: String,
    /// Create a global (admin-owned) database; requires admin. Defaults to false.
    #[serde(default)]
    global: bool,
}

#[derive(Deserialize)]
struct RenameBody {
    name: String,
}

async fn list(State(state): State<AppState>, user: CurrentUser) -> Result<Response, DatabaseError> {
    let rows = service(&state).list(&user).await?;
    let views: Vec<DatabaseView> = rows.into_iter().map(DatabaseView::from).collect();
    Ok((StatusCode::OK, Json(views)).into_response())
}

async fn create(
    State(state): State<AppState>,
    user: CurrentUser,
    Json(body): Json<CreateBody>,
) -> Result<Response, DatabaseError> {
    let model = service(&state)
        .create(&user, &body.name, &body.kind, body.global)
        .await?;
    Ok((StatusCode::CREATED, Json(DatabaseView::from(model))).into_response())
}

async fn get_one(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<i32>,
) -> Result<Response, DatabaseError> {
    let model = service(&state).get(&user, id).await?;
    Ok((StatusCode::OK, Json(DatabaseView::from(model))).into_response())
}

async fn rename(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<i32>,
    Json(body): Json<RenameBody>,
) -> Result<Response, DatabaseError> {
    let model = service(&state).rename(&user, id, &body.name).await?;
    Ok((StatusCode::OK, Json(DatabaseView::from(model))).into_response())
}

async fn delete(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<i32>,
) -> Result<Response, DatabaseError> {
    service(&state).delete(&user, id).await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

fn service(state: &AppState) -> DatabaseService {
    DatabaseService::new(state.db.clone())
}

/// Map service failures onto HTTP status + a JSON error envelope. 5xx details are
/// internal; clients get a generic message (never a raw `DbErr`).
impl IntoResponse for DatabaseError {
    fn into_response(self) -> Response {
        let status = match &self {
            DatabaseError::NotFound => StatusCode::NOT_FOUND,
            DatabaseError::Forbidden => StatusCode::FORBIDDEN,
            DatabaseError::InvalidKind(_) | DatabaseError::InvalidInput(_) => {
                StatusCode::BAD_REQUEST
            }
            DatabaseError::Db(_) => StatusCode::INTERNAL_SERVER_ERROR,
        };
        let message = match status {
            StatusCode::INTERNAL_SERVER_ERROR => "internal error".to_string(),
            _ => self.to_string(),
        };
        (status, Json(json!({ "error": message }))).into_response()
    }
}
