//! Integration tests for request identity: the `CurrentUser` extractor over the
//! HTTP layer and the shared `scope` ownership filter against a real DB.

use axum::body::Body;
use axum::http::Request;
use http_body_util::BodyExt;
use sea_orm::{ActiveModelTrait, EntityTrait, QueryFilter, Set};
use tower::ServiceExt;

use chess_base::db::entities::databases;
use chess_base::db::{connect, DbConfig};
use chess_base::server::identity::{scope, CurrentUser, LOCAL_ADMIN_ID};
use chess_base::server::{build_router, AppState, Mode};

async fn whoami(mode: Mode) -> (axum::http::StatusCode, serde_json::Value) {
    let db = connect(&DbConfig::in_memory()).await.unwrap();
    let app = build_router(AppState { db, mode });

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/api/whoami")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let body = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
    (status, body)
}

#[tokio::test]
async fn local_mode_resolves_implicit_admin() {
    let (status, body) = whoami(Mode::Local).await;
    assert_eq!(status, 200);
    assert_eq!(body["id"], LOCAL_ADMIN_ID);
    assert_eq!(body["is_admin"], true);
}

#[tokio::test]
async fn server_mode_is_unauthorized_until_auth_lands() {
    // #14 swaps in real resolution; until then server mode has no caller.
    let (status, _body) = whoami(Mode::Server).await;
    assert_eq!(status, 401);
}

#[tokio::test]
async fn scope_filter_matches_own_and_global_only() {
    let db = connect(&DbConfig::in_memory()).await.unwrap();

    for (owner, name, kind) in [
        (Some("local-admin"), "Mine", "own"),
        (Some("someone-else"), "Theirs", "own"),
        (None, "Master DB", "master"),
    ] {
        databases::ActiveModel {
            owner_id: Set(owner.map(str::to_string)),
            name: Set(name.to_string()),
            kind: Set(kind.to_string()),
            ..Default::default()
        }
        .insert(&db)
        .await
        .unwrap();
    }

    let user = CurrentUser {
        id: "local-admin".to_string(),
        is_admin: true,
    };

    let visible = databases::Entity::find()
        .filter(scope(databases::Column::OwnerId, &user))
        .all(&db)
        .await
        .unwrap();

    let names: Vec<&str> = visible.iter().map(|m| m.name.as_str()).collect();
    assert_eq!(visible.len(), 2, "caller sees own + global, not others'");
    assert!(names.contains(&"Mine"));
    assert!(names.contains(&"Master DB"));
    assert!(!names.contains(&"Theirs"));
}
