//! Integration tests for the per-user settings HTTP API: defaults, the get/set
//! round trip, persistence across a rebuilt router (same DB), validation, and the
//! server-mode auth gate.

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use axum::Router;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;

use chess_base::db::{connect, DbConfig};
use chess_base::server::{build_router, AppState, Mode};

async fn send(app: &Router, req: Request<Body>) -> (StatusCode, Value) {
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let body = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, body)
}

fn get(uri: &str) -> Request<Body> {
    Request::builder().uri(uri).body(Body::empty()).unwrap()
}

fn put(uri: &str, body: Value) -> Request<Body> {
    Request::builder()
        .method("PUT")
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap()
}

#[tokio::test]
async fn get_set_round_trip_and_persists() {
    // Local mode: the single user is the implicit admin.
    let db = connect(&DbConfig::in_memory()).await.unwrap();
    let app = build_router(AppState {
        db: db.clone(),
        mode: Mode::Local,
        engine_service: None,
        llm_provider: None,
    });

    // Defaults: an empty object before anything is stored.
    let (status, settings) = send(&app, get("/api/settings")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(settings, json!({}));

    // Create a database to reference as the default.
    let (status, created) = send(
        &app,
        put_json_post("/api/databases", json!({"name": "Mine", "kind": "own"})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let db_id = created["id"].as_i64().unwrap();

    // Save settings (board theme carries surrounding whitespace to trim).
    let (status, saved) = send(
        &app,
        put(
            "/api/settings",
            json!({
                "theme": "dark",
                "board_theme": "  blue  ",
                "default_database_id": db_id,
            }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(saved["theme"], "dark");
    assert_eq!(saved["board_theme"], "blue"); // trimmed

    // A rebuilt router over the same DB still sees the persisted settings.
    let app2 = build_router(AppState {
        db: db.clone(),
        mode: Mode::Local,
        engine_service: None,
        llm_provider: None,
    });
    let (_, settings) = send(&app2, get("/api/settings")).await;
    assert_eq!(settings["theme"], "dark");
    assert_eq!(settings["board_theme"], "blue");
    assert_eq!(settings["default_database_id"], db_id);
}

#[tokio::test]
async fn rejects_invalid_theme_and_unknown_database() {
    let db = connect(&DbConfig::in_memory()).await.unwrap();
    let app = build_router(AppState {
        db,
        mode: Mode::Local,
        engine_service: None,
        llm_provider: None,
    });

    let (status, body) = send(&app, put("/api/settings", json!({"theme": "neon"}))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body["error"].as_str().unwrap().contains("theme"));

    let (status, _) = send(
        &app,
        put("/api/settings", json!({"default_database_id": 4242})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn server_mode_requires_auth() {
    // Server mode with no credentials → the extractor rejects before the handler.
    let db = connect(&DbConfig::in_memory()).await.unwrap();
    let app = build_router(AppState {
        db,
        mode: Mode::Server,
        engine_service: None,
        llm_provider: None,
    });

    let (status, _) = send(&app, get("/api/settings")).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, _) = send(&app, put("/api/settings", json!({"theme": "dark"}))).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

/// POST helper for creating the referenced database.
fn put_json_post(uri: &str, body: Value) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap()
}
