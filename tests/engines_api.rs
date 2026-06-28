//! Integration tests for the engine-registry HTTP CRUD: list / add-edit /
//! select default / remove, persistence across a rebuilt router (same DB), and
//! the admin gate on writes in server mode.

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

fn json_req(method: &str, uri: &str, body: Value) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap()
}

#[tokio::test]
async fn crud_persists_and_default_resolves() {
    // Local mode: the single user is the implicit admin, so writes are allowed.
    let db = connect(&DbConfig::in_memory()).await.unwrap();
    let app = build_router(AppState {
        db: db.clone(),
        mode: Mode::Local,
        engine_service: None,
    });

    // Empty to start.
    let (status, list) = send(&app, get("/api/engines")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(list.as_array().unwrap().len(), 0);

    // Add two engines, the second carrying a runner wrapper.
    let (status, _) = send(
        &app,
        json_req(
            "POST",
            "/api/engines",
            json!({"name": "Stockfish", "path": "/usr/bin/stockfish"}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _) = send(
        &app,
        json_req(
            "POST",
            "/api/engines",
            json!({"name": "SF-Windows", "path": "/opt/sf.exe", "runner": "/usr/bin/wine"}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    // Both listed; the runner round-trips.
    let (_, list) = send(&app, get("/api/engines")).await;
    let arr = list.as_array().unwrap();
    assert_eq!(arr.len(), 2);
    let win = arr.iter().find(|e| e["name"] == "SF-Windows").unwrap();
    assert_eq!(win["runner"], "/usr/bin/wine");

    // The first engine became the default and resolution settles on it.
    let (status, def) = send(&app, get("/api/engines/default")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(def["default"], "Stockfish");
    assert_eq!(def["resolved"]["name"], "Stockfish");

    // Switch the default selector.
    let (status, _) = send(
        &app,
        json_req("PUT", "/api/engines/default", json!({"name": "SF-Windows"})),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    // A rebuilt router over the same DB still sees the persisted selection.
    let app2 = build_router(AppState {
        db: db.clone(),
        mode: Mode::Local,
        engine_service: None,
    });
    let (_, def) = send(&app2, get("/api/engines/default")).await;
    assert_eq!(def["default"], "SF-Windows");
    assert_eq!(def["resolved"]["runner"], "/usr/bin/wine");

    // Remove an engine.
    let (status, _) = send(
        &app2,
        Request::builder()
            .method("DELETE")
            .uri("/api/engines/Stockfish")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (_, list) = send(&app2, get("/api/engines")).await;
    assert_eq!(list.as_array().unwrap().len(), 1);

    // Removing an unknown engine is a 404.
    let (status, _) = send(
        &app2,
        Request::builder()
            .method("DELETE")
            .uri("/api/engines/ghost")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // Selecting an unknown default is a 404.
    let (status, _) = send(
        &app2,
        json_req("PUT", "/api/engines/default", json!({"name": "ghost"})),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn writes_require_admin_in_server_mode() {
    // Server mode with no credentials → the extractor rejects before the handler.
    let db = connect(&DbConfig::in_memory()).await.unwrap();
    let app = build_router(AppState {
        db,
        mode: Mode::Server,
        engine_service: None,
    });

    let (status, _) = send(
        &app,
        json_req("POST", "/api/engines", json!({"name": "X", "path": "/x"})),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}
