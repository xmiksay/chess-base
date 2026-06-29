//! Integration test: the `/mcp` DB-grounded tools (`db_position_report`,
//! `db_reference_games`), driven end-to-end via `tower::ServiceExt::oneshot`.

mod common;

use axum::http::StatusCode;
use common::{rpc_seeded, tool_json, AFTER_E4_C5, SICILIAN_PGN};
use serde_json::json;

#[tokio::test]
async fn db_position_report_tool_synthesizes_eco_and_move_stats() {
    let (status, v) = rpc_seeded(
        &[SICILIAN_PGN],
        json!({
            "jsonrpc": "2.0", "id": 20, "method": "tools/call",
            "params": { "name": "db_position_report", "arguments": { "fen": AFTER_E4_C5 } }
        }),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert!(v["result"].get("isError").is_none(), "body: {v}");
    let report = tool_json(&v["result"]);
    assert_eq!(report["eco"]["eco"], "B20");
    assert_eq!(report["total"], 1);
    let moves = report["moves"].as_array().unwrap();
    assert!(moves
        .iter()
        .any(|m| m["san"] == "Nf3" && m["frequency"] == 1.0));
}

#[tokio::test]
async fn db_reference_games_tool_returns_scoped_games() {
    let (status, v) = rpc_seeded(
        &[SICILIAN_PGN],
        json!({
            "jsonrpc": "2.0", "id": 21, "method": "tools/call",
            "params": { "name": "db_reference_games", "arguments": { "fen": AFTER_E4_C5 } }
        }),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert!(v["result"].get("isError").is_none(), "body: {v}");
    let games = tool_json(&v["result"]);
    let games = games.as_array().unwrap();
    assert_eq!(games.len(), 1);
    assert_eq!(games[0]["white"], "Tal");
    assert_eq!(games[0]["result"], "1-0");
}

#[tokio::test]
async fn db_position_report_tool_rejects_invalid_fen() {
    let (status, v) = rpc_seeded(
        &[],
        json!({
            "jsonrpc": "2.0", "id": 22, "method": "tools/call",
            "params": { "name": "db_position_report", "arguments": { "fen": "not-a-fen" } }
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
