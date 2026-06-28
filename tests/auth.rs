//! Integration tests for server-mode auth: the register/login/logout HTTP flow,
//! token + cookie resolution through `/api/whoami`, mode gating, and the
//! admin-only gate on global resources (via `StudyService`).

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use axum::Router;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;

use sea_orm::{ActiveModelTrait, Set};

use chess_base::db::entities::databases;
use chess_base::db::{connect, DbConfig};
use chess_base::server::identity::CurrentUser;
use chess_base::server::{build_router, AppState, Mode};
use chess_base::studies::{StudyError, StudyService};

async fn server_app() -> (Router, sea_orm::DatabaseConnection) {
    let db = connect(&DbConfig::in_memory()).await.unwrap();
    let app = build_router(AppState {
        db: db.clone(),
        mode: Mode::Server,
        engine: None,
        engine_service: None,
    });
    (app, db)
}

async fn send(app: &Router, req: Request<Body>) -> (StatusCode, Value) {
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let body = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, body)
}

fn post_json(uri: &str, body: Value) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap()
}

async fn register(app: &Router, username: &str, password: &str) -> (StatusCode, Value) {
    send(
        app,
        post_json(
            "/api/auth/register",
            json!({"username": username, "password": password}),
        ),
    )
    .await
}

#[tokio::test]
async fn register_login_whoami_full_flow() {
    let (app, _db) = server_app().await;

    // First user registers and becomes admin.
    let (status, body) = register(&app, "alice", "password123").await;
    assert_eq!(status, StatusCode::CREATED);
    let token = body["token"].as_str().unwrap().to_string();
    assert_eq!(body["user"]["is_admin"], true);

    // whoami with the Bearer token resolves that user.
    let (status, who) = send(
        &app,
        Request::builder()
            .uri("/api/whoami")
            .header(header::AUTHORIZATION, format!("Bearer {token}"))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(who["id"], body["user"]["id"]);
    assert_eq!(who["is_admin"], true);

    // No credentials → unauthorized.
    let (status, _) = send(
        &app,
        Request::builder()
            .uri("/api/whoami")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // A second user is a plain (non-admin) account.
    let (status, second) = register(&app, "bob", "password123").await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(second["user"]["is_admin"], false);

    // login with the right password issues a working token.
    let (status, login) = send(
        &app,
        post_json(
            "/api/auth/login",
            json!({"username": "bob", "password": "password123"}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(login["token"].as_str().is_some());

    // Wrong password is rejected.
    let (status, _) = send(
        &app,
        post_json(
            "/api/auth/login",
            json!({"username": "bob", "password": "nope"}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn duplicate_registration_conflicts() {
    let (app, _db) = server_app().await;
    register(&app, "alice", "password123").await;
    let (status, _) = register(&app, "alice", "password123").await;
    assert_eq!(status, StatusCode::CONFLICT);
}

#[tokio::test]
async fn session_cookie_authenticates() {
    let (app, _db) = server_app().await;
    let (_, body) = register(&app, "alice", "password123").await;
    let token = body["token"].as_str().unwrap();

    let (status, who) = send(
        &app,
        Request::builder()
            .uri("/api/whoami")
            .header(header::COOKIE, format!("session={token}"))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(who["is_admin"], true);
}

#[tokio::test]
async fn logout_invalidates_the_session() {
    let (app, _db) = server_app().await;
    let (_, body) = register(&app, "alice", "password123").await;
    let token = body["token"].as_str().unwrap().to_string();

    let (status, _) = send(
        &app,
        Request::builder()
            .method("POST")
            .uri("/api/auth/logout")
            .header(header::AUTHORIZATION, format!("Bearer {token}"))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let (status, _) = send(
        &app,
        Request::builder()
            .uri("/api/whoami")
            .header(header::AUTHORIZATION, format!("Bearer {token}"))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn auth_endpoints_are_disabled_in_local_mode() {
    let db = connect(&DbConfig::in_memory()).await.unwrap();
    let app = build_router(AppState {
        db,
        mode: Mode::Local,
        engine: None,
        engine_service: None,
    });
    let (status, _) = register(&app, "alice", "password123").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn only_admins_mutate_global_resources() {
    // The role resolved by auth feeds the same `assert_admin` gate every service
    // uses; exercise it through `StudyService` on a global (owner_id IS NULL) row.
    let db = connect(&DbConfig::in_memory()).await.unwrap();
    // A database row to hang the studies off (studies.database_id FK).
    let database = databases::ActiveModel {
        owner_id: Set(None),
        name: Set("Lib".into()),
        kind: Set("own".into()),
        ..Default::default()
    }
    .insert(&db)
    .await
    .unwrap();
    let db_id = database.id;
    let studies = StudyService::new(db);

    let admin = CurrentUser {
        id: "admin".into(),
        is_admin: true,
    };
    let plain = CurrentUser {
        id: "bob".into(),
        is_admin: false,
    };

    assert!(studies.create(&admin, db_id, "Global", true).await.is_ok());

    let err = studies
        .create(&plain, db_id, "Global", true)
        .await
        .unwrap_err();
    assert!(matches!(err, StudyError::Forbidden));

    // A non-admin can still create their own (non-global) study.
    assert!(studies.create(&plain, db_id, "Mine", false).await.is_ok());
}
