//! Unit tests for the MCP → entanglement tool bridge: 1:1 surface parity,
//! caller resolution in both run modes, outcome/error mapping, and the output
//! cap.

use std::collections::BTreeSet;

use super::*;
use entanglement_provider::UserId;
use entanglement_runtime::host::MAX_OUTPUT_BYTES;
use sea_orm::{ActiveModelTrait, Set};

async fn dummy_app(mode: Mode) -> AppState {
    let db = crate::db::connect(&crate::db::config::DbConfig {
        backend: crate::db::config::Backend::Sqlite {
            path: ":memory:".to_string(),
        },
    })
    .await
    .expect("connect in-memory db");
    AppState {
        db,
        mode,
        engine_service: None,
        llm_provider: None,
        provider_store: None,
    }
}

/// A [`BridgedTool`] for one named MCP tool, exactly as [`bridge_from`] builds
/// it (tests reach the private fields as a child module).
fn bridged(name: &str, app: &AppState, users: &SessionUserRegistry) -> BridgedTool {
    let mcp = Arc::new(default_registry());
    let tool = mcp
        .tools()
        .iter()
        .find(|t| t.name == name)
        .unwrap_or_else(|| panic!("MCP tool `{name}` not registered"));
    BridgedTool {
        name: tool.name,
        description: tool.description,
        schema: tool.input_schema.clone(),
        app: app.clone(),
        users: users.clone(),
        registry: Arc::clone(&mcp),
    }
}

fn text_of(parts: &[ContentPart]) -> &str {
    match parts {
        [ContentPart::Text { text }] => text,
        other => panic!("expected one text part, got {other:?}"),
    }
}

#[tokio::test]
async fn bridged_registry_mirrors_the_mcp_surface() {
    let app = dummy_app(Mode::Local).await;
    let bridged = bridge_registry(&app, &SessionUserRegistry::new());
    let mcp = default_registry();

    let mcp_names: BTreeSet<String> = mcp.tools().iter().map(|t| t.name.to_string()).collect();
    let bridged_names: BTreeSet<String> = bridged.names().into_iter().collect();
    assert_eq!(
        mcp_names.len(),
        mcp.tools().len(),
        "duplicate MCP tool name"
    );
    assert_eq!(bridged_names.len(), mcp.tools().len());
    assert_eq!(bridged_names, mcp_names);

    // Descriptions and schemas ride along 1:1.
    let echo_spec = bridged
        .specs()
        .into_iter()
        .find(|s| s.name == "echo")
        .expect("echo spec");
    let echo_mcp = mcp.tools().iter().find(|t| t.name == "echo").unwrap();
    assert_eq!(echo_spec.description, echo_mcp.description);
    assert_eq!(echo_spec.schema, echo_mcp.input_schema);
}

#[tokio::test]
async fn echo_round_trips_for_a_registered_local_session() {
    let app = dummy_app(Mode::Local).await;
    let users = SessionUserRegistry::new();
    let session = SessionId::new("s1");
    users.register(session.clone(), UserId::new("alice"));

    let tool = bridged("echo", &app, &users);
    let parts = tool
        .run_for_session(&session, "req-1", r#"{"text":"hi"}"#)
        .await
        .expect("echo succeeds");
    assert_eq!(text_of(&parts), "hi");
}

#[tokio::test]
async fn empty_input_defaults_to_an_empty_object() {
    let app = dummy_app(Mode::Local).await;
    let users = SessionUserRegistry::new();
    let session = SessionId::new("s1");
    users.register(session.clone(), UserId::new("alice"));

    // `list_databases` takes no arguments, so a schema-less empty input works.
    let tool = bridged("list_databases", &app, &users);
    let parts = tool
        .run_for_session(&session, "req-1", "")
        .await
        .expect("list_databases succeeds on empty input");
    // A fresh DB lists no databases: a JSON array, not an error.
    assert!(text_of(&parts).starts_with('['));
}

#[tokio::test]
async fn malformed_input_errors_with_the_parse_message() {
    let app = dummy_app(Mode::Local).await;
    let users = SessionUserRegistry::new();
    let session = SessionId::new("s1");
    users.register(session.clone(), UserId::new("alice"));

    let tool = bridged("echo", &app, &users);
    let err = tool
        .run_for_session(&session, "req-1", "not json")
        .await
        .expect_err("malformed input must bail");
    assert!(err.to_string().contains("parsing tool input"));
}

#[tokio::test]
async fn unregistered_session_errors() {
    let app = dummy_app(Mode::Local).await;
    let tool = bridged("echo", &app, &SessionUserRegistry::new());
    let err = tool
        .run_for_session(&SessionId::new("ghost"), "req-1", r#"{"text":"hi"}"#)
        .await
        .expect_err("unregistered session must bail");
    assert!(err.to_string().contains("no user registered"));
}

#[tokio::test]
async fn sessionless_run_is_refused() {
    let app = dummy_app(Mode::Local).await;
    let tool = bridged("echo", &app, &SessionUserRegistry::new());
    let err = tool
        .run(r#"{"text":"hi"}"#)
        .await
        .expect_err("run without a session must bail");
    assert!(err.to_string().contains("session context"));
}

#[tokio::test]
async fn tool_outcome_error_maps_to_err() {
    let app = dummy_app(Mode::Local).await;
    let users = SessionUserRegistry::new();
    let session = SessionId::new("s1");
    users.register(session.clone(), UserId::new("alice"));

    // echo without its required `text` argument yields an error outcome.
    let tool = bridged("echo", &app, &users);
    let err = tool
        .run_for_session(&session, "req-1", "{}")
        .await
        .expect_err("error outcome must map to Err");
    assert!(err.to_string().contains("missing string field `text`"));
}

#[tokio::test]
async fn oversized_output_is_truncated() {
    let app = dummy_app(Mode::Local).await;
    let users = SessionUserRegistry::new();
    let session = SessionId::new("s1");
    users.register(session.clone(), UserId::new("alice"));

    let big = "a".repeat(2 * MAX_OUTPUT_BYTES);
    let input = serde_json::to_string(&serde_json::json!({ "text": big })).unwrap();
    let tool = bridged("echo", &app, &users);
    let parts = tool
        .run_for_session(&session, "req-1", &input)
        .await
        .expect("echo succeeds");
    let text = text_of(&parts);
    // `truncate_output` caps the payload at MAX_OUTPUT_BYTES, then appends a
    // short "[truncated: N bytes total]" notice.
    assert!(text.len() <= MAX_OUTPUT_BYTES + 64, "len {}", text.len());
    assert!(text.contains("[truncated:"));
}

#[tokio::test]
async fn server_mode_resolves_the_registered_user_row() {
    let app = dummy_app(Mode::Server).await;
    users::ActiveModel {
        id: Set("u1".to_string()),
        username: Set("alice".to_string()),
        password_hash: Set("argon2-hash".to_string()),
        is_admin: Set(false),
        created_at: Set(chrono::Utc::now().naive_utc()),
    }
    .insert(&app.db)
    .await
    .expect("insert user");

    let users_reg = SessionUserRegistry::new();
    let known = SessionId::new("known");
    let unknown = SessionId::new("unknown-user");
    users_reg.register(known.clone(), UserId::new("u1"));
    users_reg.register(unknown.clone(), UserId::new("nobody"));

    let tool = bridged("echo", &app, &users_reg);
    let parts = tool
        .run_for_session(&known, "req-1", r#"{"text":"hi"}"#)
        .await
        .expect("registered user round-trips");
    assert_eq!(text_of(&parts), "hi");

    let err = tool
        .run_for_session(&unknown, "req-1", r#"{"text":"hi"}"#)
        .await
        .expect_err("a user id with no row must bail");
    assert!(err.to_string().contains("unknown user"));
}
