//! Service-level tests for the study sharing flag (issue #211, ADR-0045) over
//! an in-memory SQLite DB: the `read_scope` condition shape, the anonymous
//! read tier (public rows only — never the global arm), and the `set_public`
//! write guard. Own file: `tests.rs` is already over the file-size cap.

use super::*;
use crate::db::entities::databases;
use crate::db::{connect, DbConfig};
use sea_orm::{ActiveModelTrait, Set};

fn user(id: &str) -> CurrentUser {
    CurrentUser {
        id: id.to_string(),
        is_admin: false,
        public: false,
        read_only: false,
        global_only: false,
    }
}

/// Fresh DB with one owned games database; returns the service and that db's id.
async fn setup() -> (StudyService, i32) {
    let conn = connect(&DbConfig::in_memory()).await.unwrap();
    let db = databases::ActiveModel {
        owner_id: Set(Some("alice".to_string())),
        name: Set("Alice's games".to_string()),
        kind: Set("own".to_string()),
        ..Default::default()
    }
    .insert(&conn)
    .await
    .unwrap();
    (StudyService::new(conn), db.id)
}

#[test]
fn read_scope_for_the_anonymous_caller_is_the_public_arm_only() {
    let cond = read_scope(&CurrentUser::anonymous());
    let sql = format!("{cond:?}");
    assert!(sql.contains("public"), "missing public arm: {sql}");
    // Neither the caller-id equality nor the global IS-NULL arm may appear —
    // ADR-0043 keeps global studies off the anonymous tier.
    assert!(!sql.contains("owner_id"), "leaked an owner arm: {sql}");
}

#[test]
fn read_scope_for_an_authenticated_caller_keeps_ownership_and_adds_public() {
    let cond = read_scope(&user("alice"));
    let sql = format!("{cond:?}");
    assert!(sql.contains("alice"), "missing own-rows arm: {sql}");
    assert!(sql.contains("public"), "missing public arm: {sql}");
}

#[tokio::test]
async fn anonymous_reads_a_public_study_but_never_a_private_or_global_one() {
    let (svc, db_id) = setup().await;
    let alice = user("alice");
    let admin = CurrentUser {
        is_admin: true,
        ..user("root")
    };
    let anon = CurrentUser::anonymous();

    let private = svc.create(&alice, db_id, "Private", false).await.unwrap();
    let shared = svc.create(&alice, db_id, "Shared", false).await.unwrap();
    let global = svc.create(&admin, db_id, "Global", true).await.unwrap();
    svc.set_public(&alice, shared.id, true).await.unwrap();

    // The public study is readable anonymously; the private one is hidden.
    assert_eq!(svc.get(&anon, shared.id).await.unwrap().name, "Shared");
    assert!(matches!(
        svc.get(&anon, private.id).await.unwrap_err(),
        StudyError::NotFound
    ));
    // A global (owner NULL) study is NOT on the anonymous tier (ADR-0043)...
    assert!(matches!(
        svc.get(&anon, global.id).await.unwrap_err(),
        StudyError::NotFound
    ));
    // ...but stays visible to a signed-in user, alongside the public row.
    let bob = user("bob");
    assert!(svc.get(&bob, global.id).await.is_ok());
    assert!(svc.get(&bob, shared.id).await.is_ok());
}

#[tokio::test]
async fn set_public_enforces_the_write_guard() {
    let (svc, db_id) = setup().await;
    let alice = user("alice");
    let study = svc.create(&alice, db_id, "Line", false).await.unwrap();

    // Owner toggles freely, both ways.
    assert!(svc.set_public(&alice, study.id, true).await.unwrap().public);
    assert!(
        !svc.set_public(&alice, study.id, false)
            .await
            .unwrap()
            .public
    );

    // A stranger and the anonymous caller are denied.
    assert!(matches!(
        svc.set_public(&user("bob"), study.id, true)
            .await
            .unwrap_err(),
        StudyError::Forbidden
    ));
    assert!(matches!(
        svc.set_public(&CurrentUser::anonymous(), study.id, true)
            .await
            .unwrap_err(),
        StudyError::Forbidden
    ));

    // A read_only-scoped caller (ADR-0044) is denied even on their own study.
    let scoped = CurrentUser {
        read_only: true,
        ..user("alice")
    };
    assert!(matches!(
        svc.set_public(&scoped, study.id, true).await.unwrap_err(),
        StudyError::Forbidden
    ));
}

#[tokio::test]
async fn studies_for_game_shows_anonymous_only_the_public_analyses() {
    let (svc, db_id) = setup().await;
    let alice = user("alice");
    let conn = svc.db.clone();

    // Two studies linked to the same (fictional) game id: one public, one not.
    let mk = |name: &str| studies::ActiveModel {
        database_id: Set(db_id),
        owner_id: Set(Some("alice".to_string())),
        name: Set(name.to_string()),
        tree_json: Set(serde_json::to_string(&MoveTree::new()).unwrap()),
        origin_game_id: Set(Some(42)),
        ..Default::default()
    };
    mk("private analysis").insert(&conn).await.unwrap();
    let shared = mk("shared analysis").insert(&conn).await.unwrap();
    svc.set_public(&alice, shared.id, true).await.unwrap();

    let anon_rows = svc
        .studies_for_game(&CurrentUser::anonymous(), 42)
        .await
        .unwrap();
    assert_eq!(anon_rows.len(), 1);
    assert_eq!(anon_rows[0].name, "shared analysis");

    let own_rows = svc.studies_for_game(&alice, 42).await.unwrap();
    assert_eq!(own_rows.len(), 2);
}
