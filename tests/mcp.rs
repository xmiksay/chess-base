//! Integration test: the `/mcp` JSON-RPC endpoint, driven end-to-end via
//! `tower::ServiceExt::oneshot` — initialize → tools/list → tools/call.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;

use chess_base::db::{connect, DbConfig};
use chess_base::engine::{EngineConfig, EngineService};
use chess_base::server::auth::ensure_local_service_token;
use chess_base::server::{build_router, AppState, Mode};

/// POST a JSON-RPC request to `/mcp` (no engine configured) and return
/// (status, parsed-or-null body). Carries the seeded local service token.
async fn rpc(body: Value) -> (StatusCode, Value) {
    rpc_with_engine(body, None).await
}

/// As [`rpc`], but allows wiring a pooled engine service onto the app state so
/// the `engine_analyse` tool can be exercised.
async fn rpc_with_engine(
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

/// Path to a real UCI engine, or `None` to skip (mirrors `tests/engine.rs`).
fn engine_path() -> Option<String> {
    match std::env::var("CHESS_BASE_TEST_ENGINE") {
        Ok(p) if !p.trim().is_empty() => Some(p),
        _ => {
            eprintln!("skipping: set CHESS_BASE_TEST_ENGINE to a UCI engine binary to run");
            None
        }
    }
}

#[tokio::test]
async fn initialize_reports_protocol_server_info_and_capabilities() {
    let (status, v) = rpc(json!({
        "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {}
    }))
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(v["jsonrpc"], "2.0");
    assert_eq!(v["id"], 1);
    assert_eq!(v["result"]["protocolVersion"], "2025-03-26");
    assert_eq!(v["result"]["serverInfo"]["name"], "chess-base");
    assert!(v["result"]["capabilities"]["tools"].is_object());
    // Instructions document the engine/DB tools and board directives.
    let instructions = v["result"]["instructions"].as_str().unwrap();
    assert!(instructions.contains("<fen>"));
    assert!(instructions.contains("<pgn"));
}

#[tokio::test]
async fn tools_list_returns_the_stub_tool() {
    let (status, v) = rpc(json!({
        "jsonrpc": "2.0", "id": 2, "method": "tools/list"
    }))
    .await;

    assert_eq!(status, StatusCode::OK);
    let tools = v["result"]["tools"].as_array().unwrap();
    assert!(tools.iter().any(|t| t["name"] == "echo"));
}

#[tokio::test]
async fn tools_call_dispatches_to_the_stub_tool() {
    let (status, v) = rpc(json!({
        "jsonrpc": "2.0", "id": 3, "method": "tools/call",
        "params": { "name": "echo", "arguments": { "text": "ping" } }
    }))
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(v["result"]["content"][0]["text"], "ping");
    assert!(v["result"].get("isError").is_none());
}

#[tokio::test]
async fn notifications_initialized_is_accepted_without_body() {
    let (status, _v) = rpc(json!({
        "jsonrpc": "2.0", "method": "notifications/initialized"
    }))
    .await;

    assert_eq!(status, StatusCode::ACCEPTED);
}

#[tokio::test]
async fn unknown_method_returns_method_not_found() {
    let (_status, v) = rpc(json!({
        "jsonrpc": "2.0", "id": 4, "method": "does/not/exist"
    }))
    .await;

    assert_eq!(v["error"]["code"], -32601);
}

#[tokio::test]
async fn unknown_tool_returns_invalid_params() {
    let (_status, v) = rpc(json!({
        "jsonrpc": "2.0", "id": 5, "method": "tools/call",
        "params": { "name": "ghost", "arguments": {} }
    }))
    .await;

    assert_eq!(v["error"]["code"], -32602);
}

#[tokio::test]
async fn tools_list_includes_the_engine_tool() {
    let (status, v) = rpc(json!({
        "jsonrpc": "2.0", "id": 6, "method": "tools/list"
    }))
    .await;

    assert_eq!(status, StatusCode::OK);
    let tools = v["result"]["tools"].as_array().unwrap();
    assert!(tools.iter().any(|t| t["name"] == "engine_analyse"));
}

#[tokio::test]
async fn engine_analyse_without_engine_is_a_tool_error() {
    // No engine wired ⇒ the tool returns a graceful `isError`, not a transport
    // error. The dispatch itself still succeeds.
    let (status, v) = rpc(json!({
        "jsonrpc": "2.0", "id": 7, "method": "tools/call",
        "params": {
            "name": "engine_analyse",
            "arguments": { "fen": chess_base::position::STARTPOS_FEN }
        }
    }))
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(v["result"]["isError"], json!(true));
    assert!(v["result"]["content"][0]["text"]
        .as_str()
        .unwrap()
        .contains("No engine configured"));
}

#[tokio::test]
async fn engine_analyse_missing_fen_is_a_tool_error() {
    // Even with an engine present this would reject; with none it still must.
    let (status, v) = rpc(json!({
        "jsonrpc": "2.0", "id": 8, "method": "tools/call",
        "params": { "name": "engine_analyse", "arguments": {} }
    }))
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(v["result"]["isError"], json!(true));
}

#[tokio::test]
async fn missing_bearer_is_unauthorized_with_resource_metadata() {
    let db = connect(&DbConfig::in_memory()).await.unwrap();
    let app = build_router(AppState {
        db,
        mode: Mode::Server,
        engine_service: None,
    });

    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/mcp")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::to_vec(&json!({
                        "jsonrpc": "2.0", "id": 1, "method": "tools/list"
                    }))
                    .unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    let www = resp
        .headers()
        .get("www-authenticate")
        .expect("WWW-Authenticate header")
        .to_str()
        .unwrap();
    assert!(www.starts_with("Bearer "), "got: {www}");
    assert!(www.contains("resource_metadata="));
    assert!(www.contains("/.well-known/oauth-protected-resource"));
}

#[tokio::test]
async fn invalid_bearer_is_unauthorized() {
    let db = connect(&DbConfig::in_memory()).await.unwrap();
    let app = build_router(AppState {
        db,
        mode: Mode::Server,
        engine_service: None,
    });

    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/mcp")
                .header("content-type", "application/json")
                .header("authorization", "Bearer not-a-real-token")
                .body(Body::from(
                    serde_json::to_vec(&json!({
                        "jsonrpc": "2.0", "id": 1, "method": "tools/list"
                    }))
                    .unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

/// Acceptance: a `tools/call` mutating a study the caller does not own is
/// rejected. Alice owns the study; Bob (a different service-token identity) tries
/// to add a move through `/mcp` and gets a `not permitted` tool error.
#[tokio::test]
async fn tools_call_rejects_mutating_a_non_owned_study() {
    use chess_base::db::entities::{databases, service_tokens};
    use chess_base::server::identity::CurrentUser;
    use chess_base::studies::StudyService;
    use chrono::Utc;
    use sea_orm::{ActiveModelTrait, Set};

    let db = connect(&DbConfig::in_memory()).await.unwrap();

    let database = databases::ActiveModel {
        owner_id: Set(Some("alice".into())),
        name: Set("Alice DB".into()),
        kind: Set("own".into()),
        ..Default::default()
    }
    .insert(&db)
    .await
    .unwrap();

    let alice = CurrentUser {
        id: "alice".into(),
        is_admin: false,
    };
    let study = StudyService::new(db.clone())
        .create(&alice, database.id, "Alice's study", false)
        .await
        .unwrap();

    // Bob authenticates to /mcp with his own service token.
    let bob_token = "bob-service-token".to_string();
    service_tokens::ActiveModel {
        token: Set(bob_token.clone()),
        owner_id: Set("bob".into()),
        is_admin: Set(false),
        label: Set("bob".into()),
        created_at: Set(Utc::now().naive_utc()),
        expires_at: Set(None),
    }
    .insert(&db)
    .await
    .unwrap();

    let app = build_router(AppState {
        db,
        mode: Mode::Server,
        engine_service: None,
    });

    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/mcp")
                .header("content-type", "application/json")
                .header("authorization", format!("Bearer {bob_token}"))
                .body(Body::from(
                    serde_json::to_vec(&json!({
                        "jsonrpc": "2.0", "id": 1, "method": "tools/call",
                        "params": {
                            "name": "study_add_move",
                            "arguments": { "study_id": study.id, "from_node_id": 0, "san": "e4" }
                        }
                    }))
                    .unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let v: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(v["result"]["isError"], json!(true), "body: {v}");
    assert!(v["result"]["content"][0]["text"]
        .as_str()
        .unwrap()
        .contains("not permitted"));
}

/// End-to-end MCP analysis against a real engine: the interactive facade returns
/// an eval/PV/bestmove to a connected client. Gated on `CHESS_BASE_TEST_ENGINE`.
#[tokio::test]
async fn engine_analyse_returns_analysis_to_an_mcp_client() {
    let Some(path) = engine_path() else { return };
    let service = Arc::new(EngineService::new(EngineConfig::new("test", path), 1));

    let (status, v) = rpc_with_engine(
        json!({
            "jsonrpc": "2.0", "id": 9, "method": "tools/call",
            "params": {
                "name": "engine_analyse",
                "arguments": { "fen": chess_base::position::STARTPOS_FEN, "depth": 10 }
            }
        }),
        Some(service),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert!(
        v["result"].get("isError").is_none(),
        "analysis should succeed: {v}"
    );
    let text = v["result"]["content"][0]["text"].as_str().unwrap();
    let analysis: Value = serde_json::from_str(text).unwrap();
    assert!(analysis["bestmove"].as_str().is_some_and(|m| !m.is_empty()));
    assert!(
        analysis["score"].is_object(),
        "expected an eval: {analysis}"
    );
}
