//! Integration tests for scoped service tokens (ADR-0044, issue #193):
//! `read_only` cannot call a mutating tool, and `global_read` cannot read a
//! row owned by someone else — only global rows.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use chrono::Utc;
use http_body_util::BodyExt;
use sea_orm::{ActiveModelTrait, Set};
use serde_json::{json, Value};
use tower::ServiceExt;

use chess_base::db::entities::{databases, service_tokens};
use chess_base::db::{connect, DbConfig};
use chess_base::server::auth::{
    SERVICE_SCOPE_FULL, SERVICE_SCOPE_GLOBAL_READ, SERVICE_SCOPE_READ_ONLY,
};
use chess_base::server::{build_router, AppState, Mode};

/// Insert a service token row for `owner_id` with the given `scope` and
/// return its bearer secret.
async fn seed_token(db: &sea_orm::DatabaseConnection, owner_id: &str, scope: &str) -> String {
    let token = format!("{owner_id}-{scope}-token");
    service_tokens::ActiveModel {
        token: Set(token.clone()),
        id: Set(token.clone()),
        owner_id: Set(owner_id.to_string()),
        is_admin: Set(false),
        scope: Set(scope.to_string()),
        label: Set(format!("{owner_id}-{scope}")),
        created_at: Set(Utc::now().naive_utc()),
        expires_at: Set(None),
    }
    .insert(db)
    .await
    .unwrap();
    token
}

async fn mcp_call(app: &axum::Router, bearer: &str, body: Value) -> (StatusCode, Value) {
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/mcp")
                .header("content-type", "application/json")
                .header("authorization", format!("Bearer {bearer}"))
                .body(Body::from(serde_json::to_vec(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    (status, serde_json::from_slice(&bytes).unwrap())
}

#[tokio::test]
async fn read_only_token_can_list_but_not_call_a_mutating_tool() {
    let db = connect(&DbConfig::in_memory()).await.unwrap();
    let token = seed_token(&db, "alice", SERVICE_SCOPE_READ_ONLY).await;
    let app = build_router(AppState {
        db,
        mode: Mode::Server,
        engine_service: None,
        provider_store: None,
        agent: Default::default(),
    });

    // tools/list still works and reflects the reduced surface.
    let (status, v) = mcp_call(
        &app,
        &token,
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
    assert!(names.contains(&"list_databases"));
    assert!(
        !names.contains(&"folder_create"),
        "a gated tool must not be listed for a read_only caller"
    );

    // A read tool still works.
    let (status, v) = mcp_call(
        &app,
        &token,
        json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/call",
                "params": { "name": "list_databases", "arguments": {} } }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(v.get("error").is_none());

    // A mutating tool is rejected before ever reaching the handler.
    let (status, v) = mcp_call(
        &app,
        &token,
        json!({ "jsonrpc": "2.0", "id": 3, "method": "tools/call",
                "params": { "name": "folder_create", "arguments": { "name": "x" } } }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(v["error"]["code"], -32001);
    assert!(v["error"]["message"]
        .as_str()
        .unwrap()
        .contains("read-only"));
}

#[tokio::test]
async fn global_read_token_cannot_see_another_owners_database() {
    let db = connect(&DbConfig::in_memory()).await.unwrap();

    // A global database (visible to everyone) and one privately owned by bob.
    databases::ActiveModel {
        owner_id: Set(None),
        name: Set("Global DB".into()),
        kind: Set("master".into()),
        ..Default::default()
    }
    .insert(&db)
    .await
    .unwrap();
    databases::ActiveModel {
        owner_id: Set(Some("bob".into())),
        name: Set("Bob's DB".into()),
        kind: Set("own".into()),
        ..Default::default()
    }
    .insert(&db)
    .await
    .unwrap();

    let global_read_token = seed_token(&db, "someone-else", SERVICE_SCOPE_GLOBAL_READ).await;
    let full_token_for_bob = seed_token(&db, "bob", SERVICE_SCOPE_FULL).await;

    let app = build_router(AppState {
        db,
        mode: Mode::Server,
        engine_service: None,
        provider_store: None,
        agent: Default::default(),
    });

    let (status, v) = mcp_call(
        &app,
        &global_read_token,
        json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call",
                "params": { "name": "list_databases", "arguments": {} } }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let text = v["result"]["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("Global DB"));
    assert!(
        !text.contains("Bob's DB"),
        "global_read must never see another owner's database: {text}"
    );

    // Bob's own full-scope token still sees his own database.
    let (status, v) = mcp_call(
        &app,
        &full_token_for_bob,
        json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/call",
                "params": { "name": "list_databases", "arguments": {} } }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let text = v["result"]["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("Global DB"));
    assert!(text.contains("Bob's DB"));
}
