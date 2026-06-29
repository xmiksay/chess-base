//! Integration test: the `/mcp` `analyse_position` interactive facade (#33) —
//! DB stats + feature tags (+ engine eval when wired), driven end-to-end via
//! `tower::ServiceExt::oneshot`.

mod common;

use std::sync::Arc;

use axum::http::StatusCode;
use chess_base::engine::{EngineConfig, EngineService};
use common::{engine_path, rpc, rpc_seeded, rpc_with_engine, tool_json, AFTER_E4_C5, SICILIAN_PGN};
use serde_json::json;

#[tokio::test]
async fn analyse_position_tool_bundles_db_stats_and_features() {
    // Interactive analysis mode (#33): one call returns DB stats + feature tags.
    // No engine is wired, so `engine` is null and a note explains the omission —
    // the explanation is still grounded on the database and the features.
    let (status, v) = rpc_seeded(
        &[SICILIAN_PGN],
        json!({
            "jsonrpc": "2.0", "id": 30, "method": "tools/call",
            "params": { "name": "analyse_position", "arguments": { "fen": AFTER_E4_C5 } }
        }),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert!(v["result"].get("isError").is_none(), "body: {v}");
    let snapshot = tool_json(&v["result"]);

    // DB stats: the seeded Sicilian classifies as B20 with Nf3 the lone reply.
    assert_eq!(snapshot["database"]["eco"]["eco"], "B20");
    assert!(snapshot["database"]["moves"]
        .as_array()
        .unwrap()
        .iter()
        .any(|m| m["san"] == "Nf3"));

    // Feature tags: factual, grounded descriptors of the position.
    assert_eq!(snapshot["features"]["side_to_move"], "white");
    assert_eq!(snapshot["features"]["phase"], "opening");
    let tags = snapshot["features"]["tags"].as_array().unwrap();
    assert!(tags.iter().any(|t| t == "White to move"));

    // No engine ⇒ null eval plus an explanatory note (not a hard error).
    assert!(snapshot["engine"].is_null());
    assert!(snapshot["notes"]
        .as_array()
        .unwrap()
        .iter()
        .any(|n| n.as_str().unwrap().contains("no engine configured")));
}

#[tokio::test]
async fn analyse_position_tool_rejects_invalid_fen() {
    let (status, v) = rpc_seeded(
        &[],
        json!({
            "jsonrpc": "2.0", "id": 31, "method": "tools/call",
            "params": { "name": "analyse_position", "arguments": { "fen": "not-a-fen" } }
        }),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(v["result"]["isError"], json!(true));
    assert!(v["result"]["content"][0]["text"]
        .as_str()
        .unwrap()
        .contains("invalid FEN"));
}

#[tokio::test]
async fn tools_list_includes_the_analysis_tool() {
    let (status, v) = rpc(json!({
        "jsonrpc": "2.0", "id": 32, "method": "tools/list"
    }))
    .await;

    assert_eq!(status, StatusCode::OK);
    let tools = v["result"]["tools"].as_array().unwrap();
    assert!(tools.iter().any(|t| t["name"] == "analyse_position"));
}

/// Acceptance (#33): with a real engine wired, `analyse_position` returns a
/// single grounded snapshot — engine eval + DB report + feature tags — so a
/// client can explain the position from tool output. Gated on
/// `CHESS_BASE_TEST_ENGINE`.
#[tokio::test]
async fn analyse_position_bundles_engine_eval_for_an_mcp_client() {
    let Some(path) = engine_path() else { return };
    let service = Arc::new(EngineService::new(EngineConfig::new("test", path), 1));

    let (status, v) = rpc_with_engine(
        json!({
            "jsonrpc": "2.0", "id": 33, "method": "tools/call",
            "params": {
                "name": "analyse_position",
                "arguments": { "fen": chess_base::position::STARTPOS_FEN, "depth": 10 }
            }
        }),
        Some(service),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert!(v["result"].get("isError").is_none(), "body: {v}");
    let snapshot = tool_json(&v["result"]);
    assert!(snapshot["engine"]["bestmove"]
        .as_str()
        .is_some_and(|m| !m.is_empty()));
    assert_eq!(snapshot["features"]["phase"], "opening");
    assert!(snapshot["database"].is_object());
}
