//! HTTP surface for the study sharing toggle (issue #211, ADR-0045):
//! `PUT /api/studies/{id}/public`, a thin caller over
//! [`StudyService::set_public`]. Split out of `routes.rs` (already over the
//! file-size cap), mirroring `clear_shapes_route.rs`'s own small router.

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::put,
    Json, Router,
};
use serde::Deserialize;

use crate::server::identity::CurrentUser;
use crate::server::state::AppState;
use crate::studies::routes::StudyView;
use crate::studies::{StudyError, StudyService};

/// Body for `PUT /api/studies/{id}/public`: the new sharing state.
#[derive(Deserialize)]
struct PublicBody {
    public: bool,
}

/// Sharing-toggle route, merged into the main API router.
pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/api/studies/{id}/public", put(set_public))
        .with_state(state)
}

/// Toggle a study's public sharing flag (`PUT /api/studies/{id}/public`, issue
/// #211). The usual study write guard applies (owner, or admin for a global
/// study); returns the refreshed [`StudyView`].
async fn set_public(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<i32>,
    Json(body): Json<PublicBody>,
) -> Result<Response, StudyError> {
    let model = StudyService::new(state.db.clone())
        .set_public(&user, id, body.public)
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
    async fn a_missing_study_is_not_found() {
        let response = app()
            .await
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/api/studies/9999/public")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"public":true}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn rejects_a_body_without_the_flag() {
        let response = app()
            .await
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/api/studies/1/public")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }
}
