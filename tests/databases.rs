//! Integration tests for database (collection) CRUD over the HTTP layer in
//! server mode: the list/create/rename/delete endpoints and the scoping rules
//! (own vs global vs other-user), exercised end-to-end through real auth tokens.

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use axum::Router;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;

use chess_base::db::{connect, DbConfig};
use chess_base::server::{build_router, AppState, Mode};

async fn server_app() -> Router {
    let db = connect(&DbConfig::in_memory()).await.unwrap();
    build_router(AppState {
        db,
        mode: Mode::Server,
        engine: None,
        engine_service: None,
    })
}

async fn send(app: &Router, req: Request<Body>) -> (StatusCode, Value) {
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let body = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, body)
}

/// Register a user and return their bearer token. The first registered user is
/// admin (auth bootstrap rule).
async fn register(app: &Router, username: &str) -> String {
    let (status, body) = send(
        app,
        Request::builder()
            .method("POST")
            .uri("/api/auth/register")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                serde_json::to_vec(&json!({"username": username, "password": "password123"}))
                    .unwrap(),
            ))
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    body["token"].as_str().unwrap().to_string()
}

fn json_req(method: &str, uri: &str, token: &str, body: Value) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(uri)
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap()
}

fn get_req(uri: &str, token: &str) -> Request<Body> {
    Request::builder()
        .uri(uri)
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap()
}

#[tokio::test]
async fn crud_and_scoping_across_users() {
    let app = server_app().await;
    let admin = register(&app, "alice").await; // first user → admin
    let bob = register(&app, "bob").await;

    // Bob creates his own database.
    let (status, bobs) = send(
        &app,
        json_req(
            "POST",
            "/api/databases",
            &bob,
            json!({"name": "Bob's games", "kind": "own"}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(bobs["global"], false);
    assert!(!bobs["owner_id"].as_str().unwrap().is_empty());
    let bobs_id = bobs["id"].as_i64().unwrap();

    // Admin creates a global (master) database.
    let (status, glob) = send(
        &app,
        json_req(
            "POST",
            "/api/databases",
            &admin,
            json!({"name": "Masters", "kind": "master", "global": true}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(glob["global"], true);
    assert_eq!(glob["owner_id"], Value::Null);
    let glob_id = glob["id"].as_i64().unwrap();

    // Bob's list sees his own DB + the global one, but not the admin's private DBs.
    let (status, list) = send(&app, get_req("/api/databases", &bob)).await;
    assert_eq!(status, StatusCode::OK);
    let names: Vec<&str> = list
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, vec!["Bob's games", "Masters"]);

    // Bob can GET the global DB (read scope) but cannot rename it (admin-only).
    let (status, _) = send(&app, get_req(&format!("/api/databases/{glob_id}"), &bob)).await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = send(
        &app,
        json_req(
            "PATCH",
            &format!("/api/databases/{glob_id}"),
            &bob,
            json!({"name": "Hacked"}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // Admin does not see Bob's private DB, and cannot mutate it either.
    let (status, _) = send(&app, get_req(&format!("/api/databases/{bobs_id}"), &admin)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = send(
        &app,
        Request::builder()
            .method("DELETE")
            .uri(format!("/api/databases/{bobs_id}"))
            .header(header::AUTHORIZATION, format!("Bearer {admin}"))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // Bob renames his own DB, then deletes it.
    let (status, renamed) = send(
        &app,
        json_req(
            "PATCH",
            &format!("/api/databases/{bobs_id}"),
            &bob,
            json!({"name": "Bob's openings"}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(renamed["name"], "Bob's openings");

    let (status, _) = send(
        &app,
        Request::builder()
            .method("DELETE")
            .uri(format!("/api/databases/{bobs_id}"))
            .header(header::AUTHORIZATION, format!("Bearer {bob}"))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    // Gone from Bob's list; the global DB remains.
    let (_, list) = send(&app, get_req("/api/databases", &bob)).await;
    let names: Vec<&str> = list
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, vec!["Masters"]);
}

#[tokio::test]
async fn non_admin_cannot_create_global_database() {
    let app = server_app().await;
    let _admin = register(&app, "alice").await;
    let bob = register(&app, "bob").await;

    let (status, _) = send(
        &app,
        json_req(
            "POST",
            "/api/databases",
            &bob,
            json!({"name": "Sneaky global", "kind": "master", "global": true}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn invalid_kind_is_rejected() {
    let app = server_app().await;
    let alice = register(&app, "alice").await;

    let (status, body) = send(
        &app,
        json_req(
            "POST",
            "/api/databases",
            &alice,
            json!({"name": "Bad", "kind": "bogus"}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body["error"].as_str().unwrap().contains("bogus"));
}

#[tokio::test]
async fn unauthenticated_requests_are_rejected() {
    let app = server_app().await;
    let (status, _) = send(
        &app,
        Request::builder()
            .uri("/api/databases")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}
