//! Service-level tests for the standalone transposition-annotation pass (issue
//! #174) over an in-memory SQLite DB: tagging, idempotency and ownership gating.

use super::*;
use crate::db::entities::databases;
use crate::db::{connect, DbConfig};
use sea_orm::{ActiveModelTrait, Set};

fn user(id: &str) -> CurrentUser {
    CurrentUser {
        id: id.to_string(),
        is_admin: false,
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

/// A study whose mainline (1.d4 d5 2.c4) and whose reversed-order sideline
/// (1.c4 d5 2.d4) both reach the same position after move 2 — the classic
/// Queen's-pawn/English transposition.
async fn seed_transposing_study(svc: &StudyService, user: &CurrentUser, db_id: i32) -> i32 {
    let study = svc.create(user, db_id, "Repertoire", false).await.unwrap();

    let d4 = svc.add_move(user, study.id, 0, "d4").await.unwrap();
    let d5 = svc.add_move(user, study.id, d4, "d5").await.unwrap();
    svc.add_move(user, study.id, d5, "c4").await.unwrap();

    let c4 = svc.add_move(user, study.id, 0, "c4").await.unwrap();
    let v_d5 = svc.add_move(user, study.id, c4, "d5").await.unwrap();
    svc.add_move(user, study.id, v_d5, "d4").await.unwrap();

    study.id
}

#[tokio::test]
async fn tags_the_transposing_line_and_refreshes_the_study() {
    let (svc, db_id) = setup().await;
    let alice = user("alice");
    let study_id = seed_transposing_study(&svc, &alice, db_id).await;

    let updated = svc.mark_transpositions(&alice, study_id).await.unwrap();
    let tree: MoveTree = serde_json::from_str(&updated.tree_json).unwrap();

    let c4 = tree.nodes[tree.root]
        .children
        .iter()
        .copied()
        .find(|&c| tree.nodes[c].san.as_deref() == Some("c4"))
        .unwrap();
    let v_d5 = tree.nodes[c4].children[0];
    let transposed = tree.nodes[v_d5].children[0];
    assert_eq!(
        tree.nodes[transposed].comment.as_deref(),
        Some("Transposes to the main line after 2.c4")
    );
}

#[tokio::test]
async fn re_running_is_idempotent() {
    let (svc, db_id) = setup().await;
    let alice = user("alice");
    let study_id = seed_transposing_study(&svc, &alice, db_id).await;

    svc.mark_transpositions(&alice, study_id).await.unwrap();
    let first = svc.get(&alice, study_id).await.unwrap().tree_json;
    svc.mark_transpositions(&alice, study_id).await.unwrap();
    let second = svc.get(&alice, study_id).await.unwrap().tree_json;
    assert_eq!(first, second);
}

#[tokio::test]
async fn ownership_and_existence_are_enforced() {
    let (svc, db_id) = setup().await;
    let alice = user("alice");
    let bob = user("bob");
    let study_id = seed_transposing_study(&svc, &alice, db_id).await;

    assert!(matches!(
        svc.mark_transpositions(&bob, study_id).await.unwrap_err(),
        StudyError::Forbidden
    ));
    assert!(matches!(
        svc.mark_transpositions(&alice, 9999).await.unwrap_err(),
        StudyError::NotFound
    ));
}
