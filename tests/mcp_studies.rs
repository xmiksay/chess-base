//! Integration test: the `/mcp` study & database tools completing the surface
//! (issue #125) — `list_databases`, `study_import_pgn`, `study_get`,
//! `study_add_move` (UCI + inline annotation), `db_list_games`, `db_read_game` —
//! driven end-to-end via `tower::ServiceExt::oneshot`. A [`common::Session`]
//! keeps one DB across calls so a write is visible to a later read.

mod common;

use std::sync::Arc;

use axum::http::StatusCode;
use chess_base::engine::{EngineConfig, EngineService};
use common::{engine_path, rpc, rpc_with_engine, seeded_session, tool_json, SICILIAN_PGN};
use serde_json::json;

#[tokio::test]
async fn list_databases_reports_the_seeded_database_with_a_game_count() {
    let session = seeded_session(&[SICILIAN_PGN]).await;
    let dbs = tool_json(&session.tool(200, "list_databases", json!({})).await);
    let dbs = dbs.as_array().unwrap();
    assert_eq!(dbs.len(), 1);
    assert_eq!(dbs[0]["name"], "Masters");
    assert_eq!(dbs[0]["global"], true);
    assert_eq!(dbs[0]["game_count"], 1);
}

#[tokio::test]
async fn db_list_and_read_game_round_trip() {
    let session = seeded_session(&[SICILIAN_PGN]).await;
    let page = tool_json(
        &session
            .tool(201, "db_list_games", json!({ "database_id": 1 }))
            .await,
    );
    let games = page["games"].as_array().unwrap();
    assert_eq!(games.len(), 1);
    assert_eq!(games[0]["white"], "Tal");
    let game_id = games[0]["id"].as_i64().unwrap();

    let game = tool_json(
        &session
            .tool(202, "db_read_game", json!({ "game_id": game_id }))
            .await,
    );
    assert_eq!(game["white"], "Tal");
    assert!(game["pgn"].as_str().unwrap().contains("c5"));
}

#[tokio::test]
async fn db_read_game_missing_id_is_a_tool_error() {
    let (status, v) = rpc(json!({
        "jsonrpc": "2.0", "id": 203, "method": "tools/call",
        "params": { "name": "db_read_game", "arguments": { "game_id": 9999 } }
    }))
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(v["result"]["isError"], json!(true));
    assert!(v["result"]["content"][0]["text"]
        .as_str()
        .unwrap()
        .contains("game not found"));
}

#[tokio::test]
async fn study_import_pgn_then_get_reads_back_the_tree_with_node_ids() {
    let session = seeded_session(&[]).await;
    let imported = tool_json(
        &session
            .tool(
                204,
                "study_import_pgn",
                json!({
                    "database_id": 1,
                    "name": "Ruy",
                    "pgn": "1. e4 e5 2. Nf3 Nc6 3. Bb5 *",
                    "global": true
                }),
            )
            .await,
    );
    let study_id = imported["id"].as_i64().unwrap();

    let view = tool_json(
        &session
            .tool(205, "study_get", json!({ "study_id": study_id }))
            .await,
    );
    assert_eq!(view["name"], "Ruy");
    let nodes = view["tree"]["nodes"].as_array().unwrap();
    // Root + 5 plies, each node carrying its own id.
    assert_eq!(nodes.len(), 6);
    assert!(nodes.iter().all(|n| n["id"].is_number()));
}

#[tokio::test]
async fn study_add_move_accepts_uci_with_inline_comment_and_returns_fen() {
    let session = seeded_session(&[]).await;
    let study = tool_json(
        &session
            .tool(
                206,
                "study_create",
                json!({ "database_id": 1, "name": "From UCI", "global": true }),
            )
            .await,
    );
    let study_id = study["id"].as_i64().unwrap();

    let added = tool_json(
        &session
            .tool(
                207,
                "study_add_move",
                json!({
                    "study_id": study_id,
                    "from_node_id": 0,
                    "uci": "g1f3",
                    "comment": "develop"
                }),
            )
            .await,
    );
    assert_eq!(added["san"], "Nf3");
    assert!(added["fen"].as_str().unwrap().contains("5N2"));

    // The inline comment landed on the new node.
    let view = tool_json(
        &session
            .tool(208, "study_get", json!({ "study_id": study_id }))
            .await,
    );
    let node_id = added["node_id"].as_u64().unwrap() as usize;
    assert_eq!(view["tree"]["nodes"][node_id]["comment"], "develop");
}

#[tokio::test]
async fn study_add_move_without_san_or_uci_is_a_tool_error() {
    let (status, v) = rpc(json!({
        "jsonrpc": "2.0", "id": 209, "method": "tools/call",
        "params": {
            "name": "study_add_move",
            "arguments": { "study_id": 1, "from_node_id": 0 }
        }
    }))
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(v["result"]["isError"], json!(true));
    assert!(v["result"]["content"][0]["text"]
        .as_str()
        .unwrap()
        .contains("`san` or `uci`"));
}

#[tokio::test]
async fn analyse_game_without_engine_is_a_tool_error() {
    let (status, v) = rpc(json!({
        "jsonrpc": "2.0", "id": 210, "method": "tools/call",
        "params": {
            "name": "analyse_game",
            "arguments": { "pgn": "1. e4 e5 2. Nf3 *" }
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

/// End-to-end `analyse_game` against a real engine: per-ply review facts plus the
/// annotated movetext. Gated on `CHESS_BASE_TEST_ENGINE`.
#[tokio::test]
async fn analyse_game_reviews_every_ply_with_a_real_engine() {
    let Some(path) = engine_path() else { return };
    let service = Arc::new(EngineService::new(EngineConfig::new("test", path), 1));

    let (status, v) = rpc_with_engine(
        json!({
            "jsonrpc": "2.0", "id": 211, "method": "tools/call",
            "params": {
                "name": "analyse_game",
                "arguments": { "pgn": "1. e4 e5 2. Nf3 Nc6 *", "depth": 8 }
            }
        }),
        Some(service),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert!(
        v["result"].get("isError").is_none(),
        "review should succeed: {v}"
    );
    let review = tool_json(&v["result"]);
    let moves = review["moves"].as_array().unwrap();
    assert_eq!(moves.len(), 3);
    assert!(moves[0]["classification"].is_string());
    // The annotated movetext embeds engine evals via the shared serializer.
    assert!(review["pgn"].as_str().unwrap().contains("%eval"));
}
