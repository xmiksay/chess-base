//! Unit tests for [`SessionService`] over an in-memory SQLite database and an
//! `EchoLlm` engine (the test `EngineConfig` wires no `model_resolver`, so the
//! `build` profile's sentinel pin is inert and no provider is ever dialed).

use super::*;
use crate::ai::agent::engine::agent_profiles;
use crate::db::config::DbConfig;
use entanglement_core::EngineConfig;
use sea_orm::ActiveModelTrait;

fn user(id: &str) -> CurrentUser {
    CurrentUser {
        id: id.to_string(),
        is_admin: false,
    }
}

/// A service whose provider store has the env-key house fallback, so
/// `create`'s provider guard passes without any DB row.
async fn service() -> (SessionService, DatabaseConnection) {
    let db = crate::db::connect(&DbConfig::in_memory())
        .await
        .expect("connect in-memory db");
    let store = AgentProviderStore::new_with_env(db.clone(), Some("test-key".to_string()))
        .await
        .expect("build provider store");
    let holly = Holly::spawn(EngineConfig {
        profiles: agent_profiles(),
        ..EngineConfig::default() // EchoLlm, no model_resolver
    });
    let users = SessionUserRegistry::new();
    (SessionService::new(db.clone(), holly, users, store), db)
}

fn out_event_json(session: &str, seq: u64, text: &str) -> String {
    serde_json::to_string(&LogPayload::Out(OutEvent::TextDelta {
        session: SessionId::new(session),
        seq,
        text: text.to_string(),
    }))
    .expect("serialize payload")
}

async fn insert_event(db: &DatabaseConnection, root: &str, ord: i64, payload: String) {
    let direction = if payload.contains("\"direction\":\"in\"") {
        "in"
    } else if payload.contains("\"dropped\"") {
        "gap"
    } else {
        "out"
    };
    agent_events::ActiveModel {
        root_session_id: Set(root.to_string()),
        user_id: Set(root.split(':').next().unwrap_or("unknown").to_string()),
        ord: Set(ord),
        ts: Set(ord),
        session_id: Set(root.to_string()),
        direction: Set(direction.to_string()),
        payload: Set(payload),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("insert agent event");
}

#[tokio::test]
async fn create_inserts_the_row_registers_the_owner_and_marks_live() {
    let (service, db) = service().await;
    let alice = user("alice");

    let root = service
        .create(
            &alice,
            "build me a repertoire".into(),
            Some("Repertoire".into()),
        )
        .await
        .expect("create session");
    assert!(root.starts_with("alice:"), "namespaced id, got {root}");

    let row = agent_sessions::Entity::find_by_id(root.clone())
        .one(&db)
        .await
        .expect("query")
        .expect("agent_sessions row exists");
    assert_eq!(row.user_id, "alice");
    assert_eq!(row.name.as_deref(), Some("Repertoire"));
    assert_eq!(row.agent, AGENT_PROFILE);

    let session = SessionId::new(root);
    assert_eq!(
        service.users.user_for(&session).map(|u| u.0),
        Some("alice".to_string())
    );
    assert!(service.is_live(&session));
}

#[tokio::test]
async fn create_without_any_provider_surface_is_a_typed_error() {
    let db = crate::db::connect(&DbConfig::in_memory())
        .await
        .expect("connect in-memory db");
    // No rows, no env key ⇒ no house context ⇒ nothing to resolve.
    let store = AgentProviderStore::new_with_env(db.clone(), None)
        .await
        .expect("build provider store");
    let holly = Holly::spawn(EngineConfig::default());
    let service = SessionService::new(db, holly, SessionUserRegistry::new(), store);

    let err = service
        .create(&user("alice"), "hi".into(), None)
        .await
        .expect_err("must refuse without a provider");
    assert!(matches!(err, SessionError::NoProvider));
}

#[tokio::test]
async fn list_is_scoped_to_the_calling_user() {
    let (service, _db) = service().await;
    let root = service
        .create(&user("alice"), "hi".into(), None)
        .await
        .expect("create");

    let mine = service.list(&user("alice")).await.expect("list");
    assert_eq!(mine.len(), 1);
    assert_eq!(mine[0].root_session_id, root);
    assert!(mine[0].live);

    let theirs = service.list(&user("bob")).await.expect("list");
    assert!(theirs.is_empty(), "user B must not see user A's sessions");
}

#[tokio::test]
async fn open_by_a_non_owner_is_not_found_even_for_an_admin() {
    let (service, _db) = service().await;
    let root = service
        .create(&user("alice"), "hi".into(), None)
        .await
        .expect("create");

    let bob = service.open(&user("bob"), &root).await;
    assert!(matches!(bob, Err(SessionError::NotFound)));

    let admin = CurrentUser {
        id: "root-admin".to_string(),
        is_admin: true,
    };
    let admin_open = service.open(&admin, &root).await;
    assert!(
        matches!(admin_open, Err(SessionError::NotFound)),
        "admin does not bypass transcript ownership"
    );
}

#[tokio::test]
async fn open_returns_the_persisted_out_events_for_a_live_session() {
    let (service, db) = service().await;
    let alice = user("alice");
    let root = service
        .create(&alice, "hi".into(), None)
        .await
        .expect("create");

    insert_event(&db, &root, 0, out_event_json(&root, 1, "first")).await;
    insert_event(&db, &root, 1, out_event_json(&root, 2, "second")).await;

    let opened = service.open(&alice, &root).await.expect("open");
    assert!(!opened.gapped);
    assert_eq!(opened.events.len(), 2);
    assert!(matches!(
        &opened.events[0],
        OutEvent::TextDelta { text, .. } if text == "first"
    ));
}

#[tokio::test]
async fn open_flags_a_gap_and_returns_only_the_intact_prefix() {
    let (service, db) = service().await;
    let alice = user("alice");
    let root = service
        .create(&alice, "hi".into(), None)
        .await
        .expect("create");

    insert_event(&db, &root, 0, out_event_json(&root, 1, "kept")).await;
    insert_event(
        &db,
        &root,
        1,
        serde_json::to_string(&LogPayload::Gap { dropped: 3 }).expect("serialize"),
    )
    .await;
    insert_event(&db, &root, 2, out_event_json(&root, 2, "lost")).await;

    let opened = service.open(&alice, &root).await.expect("open");
    assert!(opened.gapped);
    assert_eq!(opened.events.len(), 1, "everything after the gap is cut");
}

#[tokio::test]
async fn delete_removes_the_row_the_events_and_the_registrations() {
    let (service, db) = service().await;
    let alice = user("alice");
    let root = service
        .create(&alice, "hi".into(), None)
        .await
        .expect("create");
    insert_event(&db, &root, 0, out_event_json(&root, 1, "chunk")).await;

    service.delete(&alice, &root).await.expect("delete");

    assert!(agent_sessions::Entity::find_by_id(root.clone())
        .one(&db)
        .await
        .expect("query")
        .is_none());
    let events = agent_events::Entity::find()
        .filter(agent_events::Column::RootSessionId.eq(root.clone()))
        .all(&db)
        .await
        .expect("query");
    assert!(events.is_empty());
    let session = SessionId::new(root);
    assert!(service.users.user_for(&session).is_none());
    assert!(!service.is_live(&session));

    // Deleting again (or as anyone else) is NotFound, not a raw DB error.
    assert!(matches!(
        service.delete(&alice, &session.0).await,
        Err(SessionError::NotFound)
    ));
}

#[tokio::test]
async fn touch_and_rename_update_the_index_row() {
    let (service, db) = service().await;
    let alice = user("alice");
    let root = service
        .create(&alice, "hi".into(), None)
        .await
        .expect("create");

    service
        .rename(&root, "Named by the engine")
        .await
        .expect("rename");
    let row = agent_sessions::Entity::find_by_id(root.clone())
        .one(&db)
        .await
        .expect("query")
        .expect("row");
    assert_eq!(row.name.as_deref(), Some("Named by the engine"));

    let before = row.last_active;
    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    service.touch(&root).await.expect("touch");
    let row = agent_sessions::Entity::find_by_id(root)
        .one(&db)
        .await
        .expect("query")
        .expect("row");
    assert!(row.last_active >= before);

    // Unknown roots are a no-op, never an error.
    service.touch("ghost:nope").await.expect("touch unknown");
    service
        .rename("ghost:nope", "x")
        .await
        .expect("rename unknown");
}
