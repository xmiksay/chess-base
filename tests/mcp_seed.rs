//! Integration test: seeding a study straight from the `opening_tree` /
//! `danger_map` data tools via their optional `save_as` argument (issue #155). No
//! tree JSON round-trips back to the client and no language model is invoked — the
//! server builds the tree, persists it, and returns only an id + node count.

mod common;

use std::sync::Arc;

use axum::http::StatusCode;
use chess_base::engine::{EngineConfig, EngineService};
use common::{
    engine_path, rpc, seeded_session, seeded_session_with_engine, tool_json, SICILIAN_PGN,
};
use serde_json::json;

#[tokio::test]
async fn tools_list_advertises_save_as_on_the_data_tools() {
    let (status, v) = rpc(json!({ "jsonrpc": "2.0", "id": 300, "method": "tools/list" })).await;
    assert_eq!(status, StatusCode::OK);
    let tools = v["result"]["tools"].as_array().unwrap();
    for name in ["opening_tree", "danger_map"] {
        let tool = tools
            .iter()
            .find(|t| t["name"] == name)
            .unwrap_or_else(|| panic!("missing tool {name}"));
        let save_as = &tool["inputSchema"]["properties"]["save_as"];
        assert_eq!(
            save_as["required"],
            json!(["database_id", "name"]),
            "{name} should advertise the save_as seed argument"
        );
    }
}

#[tokio::test]
async fn opening_tree_save_as_with_bad_shape_is_a_tool_error() {
    // `save_as` present but missing `name` is rejected without an engine even
    // running — a malformed request is cheap to bounce. (No engine ⇒ the engine
    // gate fires first; this asserts that gate, the validation unit-tests cover the
    // parse.)
    let session = seeded_session(&[SICILIAN_PGN]).await;
    let (status, v) = session
        .call(json!({
            "jsonrpc": "2.0", "id": 301, "method": "tools/call",
            "params": {
                "name": "opening_tree",
                "arguments": { "save_as": { "database_id": 1 } }
            }
        }))
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(v["result"]["isError"], json!(true));
}

/// Acceptance (#155): with a real engine wired, `opening_tree` + `save_as` builds
/// the tree server-side, persists it as a study owned by the caller, and returns
/// just `{ study_id, node_count }` — no tree JSON. The saved study reads back via
/// the normal `study_get` path. Gated on `CHESS_BASE_TEST_ENGINE`.
#[tokio::test]
async fn opening_tree_save_as_seeds_a_persisted_study() {
    let Some(path) = engine_path() else { return };
    let service = Arc::new(EngineService::new(EngineConfig::new("test", path), 1));
    let session = seeded_session_with_engine(&[SICILIAN_PGN], Some(service)).await;

    let seeded = tool_json(
        &session
            .tool(
                302,
                "opening_tree",
                json!({
                    "engine_depth": 6,
                    "tree": { "max_depth": 2, "max_children": 3, "max_nodes": 20, "min_frequency": 0.0, "eval_margin_cp": 300 },
                    "save_as": { "database_id": 1, "name": "Seeded opening", "global": true }
                }),
            )
            .await,
    );

    // Only an id + node count — never the tree itself.
    let study_id = seeded["study_id"].as_i64().expect("study_id returned");
    assert!(seeded["node_count"].as_u64().unwrap() >= 1);
    assert!(seeded.get("tree").is_none(), "no tree JSON in the response");

    // The study exists and reads back through the normal study path.
    let view = tool_json(
        &session
            .tool(303, "study_get", json!({ "study_id": study_id }))
            .await,
    );
    assert_eq!(view["name"], "Seeded opening");
    let nodes = view["tree"]["nodes"].as_array().unwrap();
    assert_eq!(nodes.len() as u64, seeded["node_count"].as_u64().unwrap());
}
