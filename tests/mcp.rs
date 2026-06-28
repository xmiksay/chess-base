//! Integration test: the `/mcp` JSON-RPC endpoint, driven end-to-end via
//! `tower::ServiceExt::oneshot` — initialize → tools/list → tools/call.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;

use chess_base::db::{connect, DbConfig};
use chess_base::server::{build_router, AppState, Mode};

/// POST a JSON-RPC request to `/mcp` and return (status, parsed-or-null body).
async fn rpc(body: Value) -> (StatusCode, Value) {
    let db = connect(&DbConfig::in_memory()).await.unwrap();
    let app = build_router(AppState {
        db,
        mode: Mode::Local,
        engine: None,
    });

    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/mcp")
                .header("content-type", "application/json")
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
