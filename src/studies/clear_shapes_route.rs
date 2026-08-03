//! HTTP surface for the bulk shape-clear pass (issue #191):
//! `POST /api/studies/{id}/clear-shapes`, a thin caller over
//! [`StudyService::clear_shapes`]. Split out of `routes.rs` (already over the
//! file-size cap), mirroring `mark_transpositions_route.rs`'s own small router.

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::post,
    Json, Router,
};
use serde::Deserialize;

use crate::server::identity::CurrentUser;
use crate::server::state::AppState;
use crate::studies::clear_shapes::ClearShapesScope;
use crate::studies::routes::StudyView;
use crate::studies::{StudyError, StudyService};

/// Body for `POST /api/studies/{id}/clear-shapes`.
#[derive(Deserialize)]
struct ClearShapesBody {
    scope: ClearShapesScope,
}

/// Bulk shape-clear route, merged into the main API router.
pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/api/studies/{id}/clear-shapes", post(clear_shapes))
        .with_state(state)
}

/// Remove generated plan/threat arrows (`scope: "generated"`) or every shape
/// (`scope: "all"`) across a whole study in one call
/// (`POST /api/studies/{id}/clear-shapes`, issue #191). Thin caller over
/// [`StudyService::clear_shapes`]; returns the refreshed [`StudyView`].
async fn clear_shapes(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<i32>,
    Json(body): Json<ClearShapesBody>,
) -> Result<Response, StudyError> {
    let model = StudyService::new(state.db.clone())
        .clear_shapes(&user, id, body.scope)
        .await?;
    Ok((StatusCode::OK, Json(StudyView::try_from(model)?)).into_response())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{connect, DbConfig};
    use crate::server::config::Mode;
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;

    async fn app() -> Router {
        let db = connect(&DbConfig::in_memory()).await.unwrap();
        let state = AppState {
            db,
            mode: Mode::Local,
            engine_service: None,
            provider_store: None,
            agent: Default::default(),
        };
        router(state)
    }

    #[tokio::test]
    async fn rejects_an_unknown_scope() {
        let response = app()
            .await
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/studies/1/clear-shapes")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"scope":"bogus"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }

    #[tokio::test]
    async fn a_missing_study_is_not_found() {
        let response = app()
            .await
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/studies/9999/clear-shapes")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"scope":"all"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }
}
