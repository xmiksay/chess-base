//! Shared harness for the `/mcp` JSON-RPC integration tests, driven end-to-end
//! via `tower::ServiceExt::oneshot`. Split out so each `mcp*` test file stays
//! under the project's 500-line file cap.
#![allow(dead_code)]

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use serde_json::Value;
use tower::ServiceExt;

use chess_base::db::{connect, DbConfig};
use chess_base::engine::EngineService;
use chess_base::server::auth::ensure_local_service_token;
use chess_base::server::{build_router, AppState, Mode};

/// POST a JSON-RPC request to `/mcp` (no engine configured) and return
/// (status, parsed-or-null body). Carries the seeded local service token.
pub async fn rpc(body: Value) -> (StatusCode, Value) {
    rpc_with_engine(body, None).await
}

/// As [`rpc`], but allows wiring a pooled engine service onto the app state so
/// the `engine_analyse` tool can be exercised.
pub async fn rpc_with_engine(
    body: Value,
    engine_service: Option<Arc<EngineService>>,
) -> (StatusCode, Value) {
    let db = connect(&DbConfig::in_memory()).await.unwrap();
    let token = ensure_local_service_token(&db).await.unwrap();
    let app = build_router(AppState {
        db,
        mode: Mode::Local,
        engine_service,
    });

    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/mcp")
                .header("content-type", "application/json")
                .header("authorization", format!("Bearer {token}"))
                .body(Body::from(serde_json::to_vec(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, value)
}

/// POST a raw (possibly malformed) body to `/mcp` with the seeded service token
/// and return (status, parsed-or-null body). Used to exercise the JSON-RPC
/// parse/invalid-request error envelopes (issue #97).
pub async fn rpc_raw(body: &'static str) -> (StatusCode, Value) {
    let db = connect(&DbConfig::in_memory()).await.unwrap();
    let token = ensure_local_service_token(&db).await.unwrap();
    let app = build_router(AppState {
        db,
        mode: Mode::Local,
        engine_service: None,
    });

    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/mcp")
                .header("content-type", "application/json")
                .header("authorization", format!("Bearer {token}"))
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();

    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, value)
}

/// Build a local-mode app over an in-memory DB seeded with `pgns` in a global
/// database (owner `NULL`, so visible to the local-admin caller), POST `body` to
/// `/mcp` with the seeded service token, and return (status, parsed body).
pub async fn rpc_seeded(pgns: &[&str], body: Value) -> (StatusCode, Value) {
    use chess_base::db::entities::databases;
    use chess_base::ingest_pgn;
    use sea_orm::{ActiveModelTrait, Set};

    let db = connect(&DbConfig::in_memory()).await.unwrap();
    let token = ensure_local_service_token(&db).await.unwrap();
    let database = databases::ActiveModel {
        owner_id: Set(None),
        name: Set("Masters".into()),
        kind: Set("master".into()),
        ..Default::default()
    }
    .insert(&db)
    .await
    .unwrap();
    for pgn in pgns {
        ingest_pgn(&db, database.id, pgn).await.unwrap();
    }
    let app = build_router(AppState {
        db,
        mode: Mode::Local,
        engine_service: None,
    });

    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/mcp")
                .header("content-type", "application/json")
                .header("authorization", format!("Bearer {token}"))
                .body(Body::from(serde_json::to_vec(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, value)
}

/// Parse the (single) text content block of a successful `tools/call` result as JSON.
pub fn tool_json(result: &Value) -> Value {
    let text = result["content"][0]["text"].as_str().expect("text block");
    serde_json::from_str(text).expect("tool returned JSON")
}

/// Path to a real UCI engine, or `None` to skip (mirrors `tests/engine.rs`).
pub fn engine_path() -> Option<String> {
    match std::env::var("CHESS_BASE_TEST_ENGINE") {
        Ok(p) if !p.trim().is_empty() => Some(p),
        _ => {
            eprintln!("skipping: set CHESS_BASE_TEST_ENGINE to a UCI engine binary to run");
            None
        }
    }
}

pub const SICILIAN_PGN: &str =
    "[White \"Tal\"]\n[Black \"Larsen\"]\n[Result \"1-0\"]\n\n1. e4 c5 2. Nf3 d6 3. d4 cxd4 1-0\n";
// 1. e4 c5 — the Sicilian Defense (ECO B20).
pub const AFTER_E4_C5: &str = "rnbqkbnr/pp1ppppp/8/2p5/4P3/8/PPPP1PPP/RNBQKBNR w KQkq - 0 2";
