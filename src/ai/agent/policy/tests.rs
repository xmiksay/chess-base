//! Unit tests for [`AgentPolicy`]: gating parity with `requires_approval`,
//! session- and always-scoped grant upgrades, and the fail-safe DB-error path.

use super::*;
use crate::ai::agent::requires_approval;
use crate::db::config::DbConfig;
use crate::server::routes::mcp::default_registry;
use entanglement_provider::UserId;
use sea_orm::ConnectionTrait;

async fn db() -> DatabaseConnection {
    crate::db::connect(&DbConfig::in_memory())
        .await
        .expect("connect in-memory db")
}

#[tokio::test]
async fn base_profile_matches_requires_approval_for_every_registered_tool() {
    let policy = AgentPolicy::new(db().await, SessionUserRegistry::new());
    let session = SessionId::new("s1");
    let registry = default_registry();
    for tool in registry.tools() {
        let expected = if requires_approval(tool.name) {
            Permission::Ask
        } else {
            Permission::Allow
        };
        assert_eq!(
            policy.resolve(&session, tool.name, "{}").await,
            expected,
            "grade mismatch for `{}`",
            tool.name
        );
    }
    // Every gated name is a real registered tool and grades Ask (all 18).
    for tool in GATED_TOOLS {
        assert!(
            registry.tools().iter().any(|t| t.name == *tool),
            "gated tool `{tool}` is not in the MCP registry"
        );
        assert_eq!(policy.resolve(&session, tool, "{}").await, Permission::Ask);
    }
    // Spot-check a read tool.
    assert_eq!(
        policy.resolve(&session, "study_get", "{}").await,
        Permission::Allow
    );
}

#[tokio::test]
async fn always_grant_upgrades_only_that_users_sessions() {
    let users = SessionUserRegistry::new();
    let alice_s = SessionId::new("alice-s");
    let bob_s = SessionId::new("bob-s");
    users.register(alice_s.clone(), UserId::new("alice"));
    users.register(bob_s.clone(), UserId::new("bob"));
    let conn = db().await;
    let policy = AgentPolicy::new(conn.clone(), users.clone());

    assert_eq!(
        policy.resolve(&alice_s, "study_create", "{}").await,
        Permission::Ask
    );
    policy
        .record(&alice_s, "study_create", None, ApprovalScope::Always)
        .await;
    assert_eq!(
        policy.resolve(&alice_s, "study_create", "{}").await,
        Permission::Allow
    );
    // Another user's session stays gated.
    assert_eq!(
        policy.resolve(&bob_s, "study_create", "{}").await,
        Permission::Ask
    );
    // A second session of the same user is upgraded too.
    let alice_s2 = SessionId::new("alice-s2");
    users.register(alice_s2.clone(), UserId::new("alice"));
    assert_eq!(
        policy.resolve(&alice_s2, "study_create", "{}").await,
        Permission::Allow
    );
    // Persisted: a fresh policy over the same DB still sees the grant.
    let fresh = AgentPolicy::new(conn.clone(), users.clone());
    assert_eq!(
        fresh.resolve(&alice_s, "study_create", "{}").await,
        Permission::Allow
    );
    // Recording the identical grant again is idempotent (NULL arg included).
    policy
        .record(&alice_s, "study_create", None, ApprovalScope::Always)
        .await;
    let rows = agent_grants::Entity::find().all(&conn).await.unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].user_id, "alice");
    assert_eq!(rows[0].scope, "always");
    assert_eq!(rows[0].arg, None);
}

#[tokio::test]
async fn always_grant_from_an_unregistered_session_is_skipped() {
    let conn = db().await;
    let policy = AgentPolicy::new(conn.clone(), SessionUserRegistry::new());
    policy
        .record(
            &SessionId::new("ghost"),
            "study_create",
            None,
            ApprovalScope::Always,
        )
        .await;
    let rows = agent_grants::Entity::find().all(&conn).await.unwrap();
    assert!(rows.is_empty());
}

#[tokio::test]
async fn session_grant_scopes_to_one_session_and_dies_with_it() {
    let policy = AgentPolicy::new(db().await, SessionUserRegistry::new());
    let s1 = SessionId::new("s1");
    let s2 = SessionId::new("s2");

    // Once records nothing.
    policy
        .record(&s1, "import_pgn", None, ApprovalScope::Once)
        .await;
    assert_eq!(
        policy.resolve(&s1, "import_pgn", "{}").await,
        Permission::Ask
    );

    policy
        .record(&s1, "import_pgn", None, ApprovalScope::Session)
        .await;
    assert_eq!(
        policy.resolve(&s1, "import_pgn", "{}").await,
        Permission::Allow
    );
    // Only that session — not a sibling.
    assert_eq!(
        policy.resolve(&s2, "import_pgn", "{}").await,
        Permission::Ask
    );
    // Nor a different tool on the granted session.
    assert_eq!(
        policy.resolve(&s1, "study_create", "{}").await,
        Permission::Ask
    );

    policy.forget_session(&s1);
    assert_eq!(
        policy.resolve(&s1, "import_pgn", "{}").await,
        Permission::Ask
    );
}

#[tokio::test]
async fn session_dir_scope_degrades_to_an_exact_session_grant() {
    let policy = AgentPolicy::new(db().await, SessionUserRegistry::new());
    let s = SessionId::new("s");
    policy
        .record(&s, "study_annotate", None, ApprovalScope::SessionDir)
        .await;
    assert_eq!(
        policy.resolve(&s, "study_annotate", "{}").await,
        Permission::Allow
    );
}

#[tokio::test]
async fn db_error_fails_safe_to_ask() {
    let conn = db().await;
    let users = SessionUserRegistry::new();
    let s = SessionId::new("s");
    users.register(s.clone(), UserId::new("alice"));
    let policy = AgentPolicy::new(conn.clone(), users);

    conn.execute_unprepared("DROP TABLE agent_grants")
        .await
        .expect("drop agent_grants");
    // The grant lookup now errors; the grade must stay Ask (never Allow/Deny).
    assert_eq!(
        policy.resolve(&s, "study_create", "{}").await,
        Permission::Ask
    );
    // A non-gated tool never hits the DB — still Allow.
    assert_eq!(
        policy.resolve(&s, "study_get", "{}").await,
        Permission::Allow
    );
}

#[tokio::test]
async fn is_granted_is_always_false() {
    let policy = AgentPolicy::new(db().await, SessionUserRegistry::new());
    let s = SessionId::new("s");
    policy
        .record(&s, "study_create", None, ApprovalScope::Session)
        .await;
    // The read side lives in the resolver; the store's own check reports
    // nothing (the trait's documented multi-tenant posture).
    assert!(!policy.is_granted(&s, "study_create", None));
}
