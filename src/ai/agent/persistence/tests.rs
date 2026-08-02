//! Unit tests for [`DbRecordSink`]/[`load_records`] over in-memory SQLite.

use super::*;
use crate::db::config::DbConfig;
use entanglement_core::{InMsg, OutEvent};
use entanglement_provider::UserId;
use sea_orm::EntityTrait;
use std::time::Duration;

async fn mem_db() -> DatabaseConnection {
    crate::db::connect(&DbConfig::in_memory())
        .await
        .expect("connect in-memory db")
}

fn in_record(session: &SessionId, text: &str) -> LogRecord {
    LogRecord::new(
        session.clone(),
        LogPayload::In(InMsg::prompt(session.clone(), text)),
    )
}

fn out_record(session: &SessionId, seq: u64, text: &str) -> LogRecord {
    LogRecord::new(
        session.clone(),
        LogPayload::Out(OutEvent::TextDelta {
            session: session.clone(),
            seq,
            text: text.to_string(),
        }),
    )
}

/// Poll `agent_events` until `root` has `n` rows (the writer task is async).
async fn wait_rows(db: &DatabaseConnection, root: &str, n: usize) -> Vec<agent_events::Model> {
    for _ in 0..250 {
        let rows = agent_events::Entity::find()
            .filter(agent_events::Column::RootSessionId.eq(root))
            .order_by_asc(agent_events::Column::Ord)
            .all(db)
            .await
            .expect("query agent_events");
        if rows.len() >= n {
            return rows;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("writer task never persisted {n} rows for {root}");
}

#[tokio::test]
async fn append_persists_rows_with_contiguous_ord_and_resolved_user() {
    let db = mem_db().await;
    let users = SessionUserRegistry::new();
    let root = SessionId::new("alice:11111111-1111-4111-8111-111111111111");
    users.register(root.clone(), UserId::new("alice"));
    let (sink, _writer) = DbRecordSink::spawn(db.clone(), users);

    sink.append(&root, &in_record(&root, "hi")).expect("append");
    sink.append(&root, &out_record(&root, 1, "chunk"))
        .expect("append");
    sink.append(
        &root,
        &LogRecord::new(root.clone(), LogPayload::Gap { dropped: 2 }),
    )
    .expect("append");

    let rows = wait_rows(&db, &root.0, 3).await;
    assert_eq!(
        rows.iter().map(|r| r.ord).collect::<Vec<_>>(),
        vec![0, 1, 2]
    );
    assert!(rows.iter().all(|r| r.user_id == "alice"));
    assert_eq!(
        rows.iter()
            .map(|r| r.direction.as_str())
            .collect::<Vec<_>>(),
        vec!["in", "out", "gap"]
    );
}

#[tokio::test]
async fn unregistered_root_falls_back_to_the_id_prefix_then_unknown() {
    let db = mem_db().await;
    let (sink, _writer) = DbRecordSink::spawn(db.clone(), SessionUserRegistry::new());

    let prefixed = SessionId::new("bob:22222222-2222-4222-8222-222222222222");
    let bare = SessionId::new("no-prefix");
    sink.append(&prefixed, &in_record(&prefixed, "hi"))
        .expect("append");
    sink.append(&bare, &in_record(&bare, "hi")).expect("append");

    assert_eq!(wait_rows(&db, &prefixed.0, 1).await[0].user_id, "bob");
    assert_eq!(wait_rows(&db, &bare.0, 1).await[0].user_id, "unknown");
}

#[tokio::test]
async fn a_new_sink_instance_continues_ord_where_the_last_stopped() {
    let db = mem_db().await;
    let root = SessionId::new("carol:33333333-3333-4333-8333-333333333333");

    let (sink, writer) = DbRecordSink::spawn(db.clone(), SessionUserRegistry::new());
    sink.append(&root, &in_record(&root, "first"))
        .expect("append");
    sink.append(&root, &out_record(&root, 1, "reply"))
        .expect("append");
    wait_rows(&db, &root.0, 2).await;
    drop(sink); // channel closes → writer exits
    writer.await.expect("writer task exits cleanly");

    // A fresh process: the in-memory counter is gone, MAX(ord) seeds it.
    let (sink, _writer) = DbRecordSink::spawn(db.clone(), SessionUserRegistry::new());
    sink.append(&root, &in_record(&root, "again"))
        .expect("append");
    let rows = wait_rows(&db, &root.0, 3).await;
    assert_eq!(
        rows.iter().map(|r| r.ord).collect::<Vec<_>>(),
        vec![0, 1, 2]
    );
}

#[tokio::test]
async fn load_records_round_trips_order_and_payloads() {
    let db = mem_db().await;
    let root = SessionId::new("dave:44444444-4444-4444-8444-444444444444");
    let child = SessionId::new("55555555-5555-4555-8555-555555555555");
    let (sink, _writer) = DbRecordSink::spawn(db.clone(), SessionUserRegistry::new());

    let written = vec![
        in_record(&root, "build me a study"),
        out_record(&root, 1, "on it"),
        // A child session's record routes into the root's log unchanged.
        out_record(&child, 1, "child chunk"),
        LogRecord::new(root.clone(), LogPayload::Gap { dropped: 4 }),
    ];
    for record in &written {
        sink.append(&root, record).expect("append");
    }
    wait_rows(&db, &root.0, written.len()).await;

    let loaded = load_records(&db, &root.0).await.expect("load_records");
    assert_eq!(loaded.len(), written.len());
    for (wrote, read) in written.iter().zip(&loaded) {
        assert_eq!(read.session, wrote.session);
        assert_eq!(read.ts, wrote.ts);
        // `LogPayload` has no `PartialEq`; JSON equality is the round-trip test.
        assert_eq!(
            serde_json::to_value(&read.payload).expect("serialize"),
            serde_json::to_value(&wrote.payload).expect("serialize"),
        );
    }
}

#[tokio::test]
async fn load_records_skips_an_undeserializable_row() {
    let db = mem_db().await;
    let root = SessionId::new("erin:66666666-6666-4666-8666-666666666666");
    let (sink, _writer) = DbRecordSink::spawn(db.clone(), SessionUserRegistry::new());
    sink.append(&root, &in_record(&root, "hi")).expect("append");
    wait_rows(&db, &root.0, 1).await;

    // A corrupt payload (as after an incompatible engine bump).
    agent_events::ActiveModel {
        root_session_id: Set(root.0.clone()),
        user_id: Set("erin".to_string()),
        ord: Set(1),
        ts: Set(0),
        session_id: Set(root.0.clone()),
        direction: Set("out".to_string()),
        payload: Set("{not json".to_string()),
        ..Default::default()
    }
    .insert(&db)
    .await
    .expect("insert corrupt row");

    let loaded = load_records(&db, &root.0).await.expect("load_records");
    assert_eq!(loaded.len(), 1, "the corrupt row is skipped, not fatal");
}
