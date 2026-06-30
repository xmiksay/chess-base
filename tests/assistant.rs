//! Integration tests for the embedded AI study assistant HTTP surface (issue
//! #20): session listing, the no-provider `503`, and the admin provider registry
//! (keys are write-only). The agent loop itself is unit-tested in
//! `ai::assistant::service` against a stub provider; here we exercise the routes
//! and error mapping over a real (in-memory) database with no live LLM.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use sea_orm::DatabaseConnection;
use serde_json::{json, Value};
use tower::ServiceExt;

use chess_base::db::{connect, DbConfig};
use chess_base::server::{build_router, AppState, Mode};

/// Send a request against a router over `db` (local mode = implicit admin, no
/// live provider). The same `db` can be reused across calls so persisted state
/// (e.g. an upserted provider) is visible to a later request.
async fn send(
    db: &DatabaseConnection,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let app = build_router(AppState {
        db: db.clone(),
        mode: Mode::Local,
        engine_service: None,
        llm_provider: None,
    });
    let builder = Request::builder().method(method).uri(uri);
    let request = match body {
        Some(b) => builder
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_vec(&b).unwrap()))
            .unwrap(),
        None => builder.body(Body::empty()).unwrap(),
    };
    let resp = app.oneshot(request).await.unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, value)
}

async fn mem_db() -> DatabaseConnection {
    connect(&DbConfig::in_memory()).await.unwrap()
}

#[tokio::test]
async fn listing_sessions_is_empty_without_a_provider() {
    let (status, body) = send(&mem_db().await, "GET", "/api/assistant/sessions", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, json!([]));
}

#[tokio::test]
async fn creating_a_session_without_a_provider_is_unavailable() {
    let (status, _) = send(
        &mem_db().await,
        "POST",
        "/api/assistant/sessions",
        Some(json!({ "title": "x" })),
    )
    .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn unknown_session_is_not_found() {
    let (status, _) = send(&mem_db().await, "GET", "/api/assistant/sessions/999", None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn provider_registry_upserts_and_never_returns_the_key() {
    let db = mem_db().await;

    // Upsert (local mode is the implicit admin).
    let (status, info) = send(
        &db,
        "POST",
        "/api/assistant/providers",
        Some(json!({
            "name": "anthropic",
            "model": "claude-sonnet-4-6",
            "api_key": "sk-super-secret",
            "is_default": true
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(info["name"], "anthropic");
    assert_eq!(info["is_default"], true);
    assert!(info.get("api_key").is_none(), "upsert echoed the key back");

    // The list reflects the upserted row but omits its key.
    let (status, list) = send(&db, "GET", "/api/assistant/providers", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(list.as_array().map(Vec::len), Some(1));
    let text = serde_json::to_string(&list).unwrap();
    assert!(
        !text.contains("super-secret"),
        "the api key leaked into the provider list"
    );
}
