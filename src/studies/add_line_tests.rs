//! Service-level tests for the "Add line to study" graft (issue #173) over an
//! in-memory SQLite DB: creating a new study, grafting into an existing one +
//! idempotency, the comment attachment, and the validation guards.

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

fn sans(moves: &[&str]) -> Vec<String> {
    moves.iter().map(|m| m.to_string()).collect()
}

#[tokio::test]
async fn creates_a_new_study_from_a_line_with_a_stats_comment() {
    let (svc, db_id) = setup().await;
    let alice = user("alice");

    let study = svc
        .add_line(
            &alice,
            &sans(&["e4", "e5", "Nf3"]),
            None,
            Some(db_id),
            Some("From the explorer".into()),
            None,
            Some("12 games, 8W/2D/2L".into()),
        )
        .await
        .unwrap();
    assert_eq!(study.database_id, db_id);
    assert_eq!(study.owner_id.as_deref(), Some("alice"));
    assert_eq!(study.name, "From the explorer");

    let tree: MoveTree = serde_json::from_str(&study.tree_json).unwrap();
    assert_eq!(tree.mainline(), vec!["e4", "e5", "Nf3"]);
    let leaf = tree
        .resolve_line(tree.root, &sans(&["e4", "e5", "Nf3"]))
        .unwrap();
    assert_eq!(
        tree.nodes[leaf].comment.as_deref(),
        Some("12 games, 8W/2D/2L")
    );
}

#[tokio::test]
async fn grafts_into_an_existing_study_and_is_idempotent() {
    let (svc, db_id) = setup().await;
    let alice = user("alice");

    let base = svc
        .create(&alice, db_id, "Repertoire", false)
        .await
        .unwrap();
    svc.add_move(&alice, base.id, 0, "d4").await.unwrap();
    let base = svc.get(&alice, base.id).await.unwrap();

    let added = svc
        .add_line(
            &alice,
            &sans(&["e4", "e5"]),
            Some(base.id),
            None,
            None,
            None,
            None,
        )
        .await
        .unwrap();
    assert_eq!(added.id, base.id);
    let tree: MoveTree = serde_json::from_str(&added.tree_json).unwrap();
    // The pre-existing d4 mainline survives; e4/e5 grafts in as a sibling variation.
    assert_eq!(tree.mainline(), vec!["d4"]);
    let first_moves: Vec<_> = tree.nodes[tree.root]
        .children
        .iter()
        .map(|&c| tree.nodes[c].san.clone().unwrap())
        .collect();
    assert_eq!(first_moves, vec!["d4", "e4"]);

    // Re-adding the same line changes nothing (SAN-follow dedup, ADR-0032).
    let again = svc
        .add_line(
            &alice,
            &sans(&["e4", "e5"]),
            Some(base.id),
            None,
            None,
            None,
            None,
        )
        .await
        .unwrap();
    assert_eq!(again.tree_json, added.tree_json);
}

#[tokio::test]
async fn rejects_empty_lines_missing_name_and_missing_database() {
    let (svc, db_id) = setup().await;
    let alice = user("alice");

    assert!(matches!(
        svc.add_line(&alice, &[], None, Some(db_id), Some("X".into()), None, None)
            .await
            .unwrap_err(),
        StudyError::InvalidEdit(_)
    ));
    assert!(matches!(
        svc.add_line(&alice, &sans(&["e4"]), None, Some(db_id), None, None, None)
            .await
            .unwrap_err(),
        StudyError::InvalidEdit(_)
    ));
    assert!(matches!(
        svc.add_line(
            &alice,
            &sans(&["e4"]),
            None,
            None,
            Some("X".into()),
            None,
            None
        )
        .await
        .unwrap_err(),
        StudyError::InvalidEdit(_)
    ));
}

#[tokio::test]
async fn rejects_an_illegal_line_and_another_users_study() {
    let (svc, db_id) = setup().await;
    let alice = user("alice");
    let bob = user("bob");

    // Nf6 is illegal as White's first move.
    assert!(matches!(
        svc.add_line(
            &alice,
            &sans(&["Nf6"]),
            None,
            Some(db_id),
            Some("X".into()),
            None,
            None
        )
        .await
        .unwrap_err(),
        StudyError::InvalidEdit(_)
    ));

    let base = svc
        .create(&alice, db_id, "Alice's study", false)
        .await
        .unwrap();
    // Bob can't write Alice's study.
    assert!(matches!(
        svc.add_line(&bob, &sans(&["e4"]), Some(base.id), None, None, None, None)
            .await
            .unwrap_err(),
        StudyError::Forbidden
    ));
}

#[tokio::test]
async fn rejects_a_set_up_start_study() {
    let (svc, db_id) = setup().await;
    let alice = user("alice");

    let base = svc
        .create(&alice, db_id, "Set-up study", false)
        .await
        .unwrap();
    let mut tree: MoveTree = MoveTree::new();
    tree.start_fen = Some("rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR b KQkq - 0 1".into());
    let mut active: studies::ActiveModel = base.clone().into();
    active.tree_json = Set(serde_json::to_string(&tree).unwrap());
    active.update(&svc.db).await.unwrap();

    assert!(matches!(
        svc.add_line(
            &alice,
            &sans(&["e5"]),
            Some(base.id),
            None,
            None,
            None,
            None
        )
        .await
        .unwrap_err(),
        StudyError::InvalidEdit(_)
    ));
}
