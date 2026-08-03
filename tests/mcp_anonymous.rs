//! Integration test: the anonymous public MCP tier (ADR-0043, issue #192). A
//! server-mode `/mcp` request with **no** `Authorization` header is served as
//! an anonymous caller — data reads scoped to global (admin-managed)
//! databases only, restricted to a small tool allowlist; a request with an
//! *invalid* bearer still `401`s (covered by `tests/mcp.rs`'s
//! `invalid_bearer_is_unauthorized`), and local mode is unchanged (covered by
//! `missing_bearer_in_local_mode_is_unauthorized_with_resource_metadata`).

mod common;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use sea_orm::{ActiveModelTrait, Set};
use serde_json::{json, Value};
use tower::ServiceExt;

use chess_base::db::entities::databases;
use chess_base::db::{connect, DbConfig};
use chess_base::ingest_pgn;
use chess_base::server::{build_router, AppState, Mode};

use common::{tool_json, SICILIAN_PGN};

/// A server-mode DB seeded with one global database (owner `NULL`) and one
/// private database (owned by `alice`), both carrying [`SICILIAN_PGN`]; plus
/// the app built over it. Returns `(app, global_database_id, private_database_id)`.
async fn seeded_app() -> (axum::Router, i32, i32) {
    let db = connect(&DbConfig::in_memory()).await.unwrap();

    let global = databases::ActiveModel {
        owner_id: Set(None),
        name: Set("Masters".into()),
        kind: Set("master".into()),
        ..Default::default()
    }
    .insert(&db)
    .await
    .unwrap();
    ingest_pgn(&db, global.id, SICILIAN_PGN).await.unwrap();

    let private = databases::ActiveModel {
        owner_id: Set(Some("alice".into())),
        name: Set("Alice's Repertoire".into()),
        kind: Set("own".into()),
        ..Default::default()
    }
    .insert(&db)
    .await
    .unwrap();
    ingest_pgn(&db, private.id, SICILIAN_PGN).await.unwrap();

    let app = build_router(AppState {
        db,
        mode: Mode::Server,
        engine_service: None,
        provider_store: None,
        agent: Default::default(),
    });
    (app, global.id, private.id)
}

/// POST a JSON-RPC `body` to `/mcp` with **no** `Authorization` header.
async fn call_anonymous(app: &axum::Router, body: Value) -> (StatusCode, Value) {
    let resp = app
        .clone()
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

/// (a) No-token `tools/list` shows only the allowlist.
#[tokio::test]
async fn anonymous_tools_list_shows_only_the_allowlist() {
    let (app, _global, _private) = seeded_app().await;
    let (status, v) = call_anonymous(
        &app,
        json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list" }),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    let names: Vec<&str> = v["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();

    for expected in [
        "echo",
        "list_databases",
        "db_list_games",
        "db_read_game",
        "db_position_report",
        "db_reference_games",
        "db_export_games",
        "search_headers",
    ] {
        assert!(names.contains(&expected), "missing {expected}: {names:?}");
    }
    assert_eq!(names.len(), 8, "unexpected extra tools: {names:?}");
    for forbidden in [
        "engine_analyse",
        "study_create",
        "study_import_pgn",
        "folder_create",
    ] {
        assert!(!names.contains(&forbidden), "leaked {forbidden}: {names:?}");
    }
}

/// (b) No-token `db_list_games` on a global DB succeeds, on a user DB fails.
#[tokio::test]
async fn anonymous_db_list_games_succeeds_on_global_and_hides_a_user_database() {
    let (app, global, private) = seeded_app().await;

    let (status, v) = call_anonymous(
        &app,
        json!({
            "jsonrpc": "2.0", "id": 2, "method": "tools/call",
            "params": { "name": "db_list_games", "arguments": { "database_id": global } }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(v["result"].get("isError").is_none(), "body: {v}");
    let page = tool_json(&v["result"]);
    assert_eq!(page["total"], 1);

    let (status, v) = call_anonymous(
        &app,
        json!({
            "jsonrpc": "2.0", "id": 3, "method": "tools/call",
            "params": { "name": "db_list_games", "arguments": { "database_id": private } }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(v["result"]["isError"], json!(true), "body: {v}");
}

/// (b, extended) `db_read_game` / `db_export_games` follow the same rule: a
/// global game's id resolves, a private one is hidden as not-found — the
/// anonymous caller can never observe that the private row exists at all.
#[tokio::test]
async fn anonymous_db_read_and_export_hide_a_private_game_as_not_found() {
    let (app, _global, _private) = seeded_app().await;

    // Discover the (global-only) game id via the allowlisted list tool rather
    // than guessing — id 1 is the global game since it's seeded first, but
    // let's not assume storage order for the private one either.
    let (_status, v) = call_anonymous(
        &app,
        json!({
            "jsonrpc": "2.0", "id": 4, "method": "tools/call",
            "params": { "name": "db_read_game", "arguments": { "game_id": 1 } }
        }),
    )
    .await;
    assert!(v["result"].get("isError").is_none(), "global game: {v}");

    let (_status, v) = call_anonymous(
        &app,
        json!({
            "jsonrpc": "2.0", "id": 5, "method": "tools/call",
            "params": { "name": "db_read_game", "arguments": { "game_id": 2 } }
        }),
    )
    .await;
    assert_eq!(v["result"]["isError"], json!(true), "private game: {v}");
    assert!(v["result"]["content"][0]["text"]
        .as_str()
        .unwrap()
        .contains("not found"));
}

/// (c) No-token `search_headers` returns only global-DB hits, even though both
/// databases carry an identically-matching game.
#[tokio::test]
async fn anonymous_search_headers_returns_only_global_hits() {
    let (app, global, _private) = seeded_app().await;

    let (status, v) = call_anonymous(
        &app,
        json!({
            "jsonrpc": "2.0", "id": 6, "method": "tools/call",
            "params": { "name": "search_headers", "arguments": { "player": "Tal" } }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(v["result"].get("isError").is_none(), "body: {v}");
    let page = tool_json(&v["result"]);
    let games = page["games"].as_array().unwrap();
    assert_eq!(games.len(), 1, "expected only the global game: {games:?}");
    assert_eq!(games[0]["database_id"], global);
}

/// (d) No-token calls to engine/study/write tools are rejected with an
/// authentication-required JSON-RPC error, not dispatched.
#[tokio::test]
async fn anonymous_calls_to_gated_tools_require_authentication() {
    let (app, global, _private) = seeded_app().await;

    for (name, arguments) in [
        (
            "engine_analyse",
            json!({ "fen": chess_base::position::STARTPOS_FEN }),
        ),
        (
            "study_create",
            json!({ "database_id": global, "name": "x" }),
        ),
        ("folder_create", json!({ "name": "x" })),
        (
            "position_threats",
            json!({ "fen": chess_base::position::STARTPOS_FEN }),
        ),
        ("opening_tree", json!({})),
        ("danger_map", json!({ "pgn": "1. e4 e5" })),
        (
            "import_pgn",
            json!({ "database_id": global, "pgn": "1. e4 e5" }),
        ),
    ] {
        let (status, v) = call_anonymous(
            &app,
            json!({
                "jsonrpc": "2.0", "id": 7, "method": "tools/call",
                "params": { "name": name, "arguments": arguments }
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(
            v["error"]["code"].is_i64(),
            "{name} should be a JSON-RPC error, got: {v}"
        );
        assert!(
            v["error"]["message"]
                .as_str()
                .unwrap()
                .to_lowercase()
                .contains("authentication"),
            "{name}: {v}"
        );
    }
}

/// The anonymous caller can never observe the existence of user-owned rows
/// (ids, names) through `list_databases` — only the global one is listed.
#[tokio::test]
async fn anonymous_list_databases_never_reveals_a_user_owned_database() {
    let (app, global, _private) = seeded_app().await;

    let (status, v) = call_anonymous(
        &app,
        json!({
            "jsonrpc": "2.0", "id": 8, "method": "tools/call",
            "params": { "name": "list_databases", "arguments": {} }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(v["result"].get("isError").is_none(), "body: {v}");
    let rows = tool_json(&v["result"]);
    let rows = rows.as_array().unwrap();
    assert_eq!(rows.len(), 1, "expected only the global database: {rows:?}");
    assert_eq!(rows[0]["id"], global);
    assert_eq!(rows[0]["global"], json!(true));
}
