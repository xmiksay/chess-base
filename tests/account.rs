//! Integration tests for account management (ADR-0055): self-service password
//! change and the admin user list + reset, through the real HTTP router.

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use axum::Router;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;

use chess_base::db::{connect, DbConfig};
use chess_base::server::{build_router, AppState, Mode};

async fn app(mode: Mode) -> Router {
    let db = connect(&DbConfig::in_memory()).await.unwrap();
    build_router(AppState {
        db,
        mode,
        engine_service: None,
        provider_store: None,
        agent: Default::default(),
        masters: None,
    })
}

async fn send(app: &Router, req: Request<Body>) -> (StatusCode, Value) {
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

fn request(method: &str, uri: &str, token: Option<&str>, body: Option<Value>) -> Request<Body> {
    let mut b = Request::builder().method(method).uri(uri);
    if let Some(t) = token {
        b = b.header(header::AUTHORIZATION, format!("Bearer {t}"));
    }
    match body {
        Some(v) => b
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(serde_json::to_vec(&v).unwrap()))
            .unwrap(),
        None => b.body(Body::empty()).unwrap(),
    }
}

/// Register a user; returns (token, user id).
async fn register(app: &Router, username: &str) -> (String, String) {
    let (status, body) = send(
        app,
        request(
            "POST",
            "/api/auth/register",
            None,
            Some(json!({"username": username, "password": "password123"})),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    (
        body["token"].as_str().unwrap().to_string(),
        body["user"]["id"].as_str().unwrap().to_string(),
    )
}

async fn login(app: &Router, username: &str, password: &str) -> StatusCode {
    let body = json!({"username": username, "password": password});
    send(app, request("POST", "/api/auth/login", None, Some(body)))
        .await
        .0
}

async fn whoami(app: &Router, token: &str) -> StatusCode {
    send(app, request("GET", "/api/whoami", Some(token), None))
        .await
        .0
}

#[tokio::test]
async fn self_service_change_keeps_the_caller_and_signs_out_other_sessions() {
    let app = app(Mode::Server).await;
    let (token, _) = register(&app, "alice").await;
    let (status, body) = send(
        &app,
        request(
            "POST",
            "/api/auth/login",
            None,
            Some(json!({"username": "alice", "password": "password123"})),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let other = body["token"].as_str().unwrap().to_string();

    let change = json!({"current_password": "password123", "new_password": "newpass456"});
    let (status, _) = send(
        &app,
        request("PUT", "/api/auth/password", Some(&token), Some(change)),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    assert_eq!(whoami(&app, &token).await, StatusCode::OK);
    assert_eq!(whoami(&app, &other).await, StatusCode::UNAUTHORIZED);
    assert_eq!(login(&app, "alice", "newpass456").await, StatusCode::OK);
    assert_eq!(
        login(&app, "alice", "password123").await,
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn self_service_change_error_statuses() {
    let app = app(Mode::Server).await;
    let (token, _) = register(&app, "alice").await;

    let wrong = json!({"current_password": "nope-nope", "new_password": "newpass456"});
    let (status, _) = send(
        &app,
        request("PUT", "/api/auth/password", Some(&token), Some(wrong)),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "wrong current ⇒ 403, not 401"
    );

    let short = json!({"current_password": "password123", "new_password": "short"});
    let (status, _) = send(
        &app,
        request("PUT", "/api/auth/password", Some(&token), Some(short)),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let ok = json!({"current_password": "password123", "new_password": "newpass456"});
    let (status, _) = send(&app, request("PUT", "/api/auth/password", None, Some(ok))).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "no session ⇒ 403");
}

#[tokio::test]
async fn admin_lists_users_and_resets_a_password() {
    let app = app(Mode::Server).await;
    let (admin, _) = register(&app, "alice").await;
    let (bob, bob_id) = register(&app, "bob").await;

    let (status, users) = send(&app, request("GET", "/api/admin/users", Some(&admin), None)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(users.as_array().unwrap().len(), 2);
    assert!(users[1].get("password_hash").is_none());

    let uri = format!("/api/admin/users/{bob_id}/password");
    let reset = json!({"new_password": "fresh-pass1"});
    let (status, _) = send(&app, request("PUT", &uri, Some(&admin), Some(reset))).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert_eq!(whoami(&app, &bob).await, StatusCode::UNAUTHORIZED);
    assert_eq!(login(&app, "bob", "fresh-pass1").await, StatusCode::OK);
}

#[tokio::test]
async fn admin_routes_reject_non_admins_and_unknown_users() {
    let app = app(Mode::Server).await;
    let (admin, admin_id) = register(&app, "alice").await;
    let (bob, _) = register(&app, "bob").await;

    let (status, _) = send(&app, request("GET", "/api/admin/users", Some(&bob), None)).await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let reset = json!({"new_password": "fresh-pass1"});
    let uri = format!("/api/admin/users/{admin_id}/password");
    let (status, _) = send(&app, request("PUT", &uri, Some(&bob), Some(reset.clone()))).await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let (status, _) = send(
        &app,
        request(
            "PUT",
            "/api/admin/users/ghost/password",
            Some(&admin),
            Some(reset),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn account_routes_are_disabled_in_local_mode() {
    let app = app(Mode::Local).await;
    let change = json!({"current_password": "password123", "new_password": "newpass456"});
    let (status, _) = send(
        &app,
        request("PUT", "/api/auth/password", None, Some(change)),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _) = send(&app, request("GET", "/api/admin/users", None, None)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}
