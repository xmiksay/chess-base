//! Service-level tests over an in-memory SQLite DB: SAN-validated `add_move`,
//! the ownership read scope, and the write guards (another user's study; a
//! global study without admin).

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

/// Deserialize a study's current move tree.
async fn tree_of(svc: &StudyService, user: &CurrentUser, id: i32) -> MoveTree {
    serde_json::from_str(&svc.get(user, id).await.unwrap().tree_json).unwrap()
}

/// Connect to a fresh in-memory DB, seed one database row, return the service
/// and that database's id.
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

#[tokio::test]
async fn add_move_validates_san_and_appends_to_tree() {
    let (svc, db_id) = setup().await;
    let alice = user("alice");

    let study = svc.create(&alice, db_id, "Openings", false).await.unwrap();
    let root = 0;

    // A legal move from the start position is accepted...
    let e4 = svc.add_move(&alice, study.id, root, "e4").await.unwrap();
    let e5 = svc.add_move(&alice, study.id, e4, "e5").await.unwrap();
    assert!(e4 > root && e5 > e4);

    // ...and a check suffix is tolerated, stored canonically.
    let reloaded = svc.get(&alice, study.id).await.unwrap();
    let tree: MoveTree = serde_json::from_str(&reloaded.tree_json).unwrap();
    assert_eq!(tree.mainline(), vec!["e4", "e5"]);

    // An illegal move in this position is rejected.
    let err = svc
        .add_move(&alice, study.id, root, "e5")
        .await
        .unwrap_err();
    assert!(matches!(err, StudyError::IllegalMove { .. }));

    // So is syntactic garbage and a bad node id.
    assert!(matches!(
        svc.add_move(&alice, study.id, root, "zz9")
            .await
            .unwrap_err(),
        StudyError::IllegalMove { .. }
    ));
    assert!(matches!(
        svc.add_move(&alice, study.id, 999, "e4").await.unwrap_err(),
        StudyError::InvalidNode(999)
    ));
}

#[tokio::test]
async fn annotate_persists_comment_and_nag() {
    let (svc, db_id) = setup().await;
    let alice = user("alice");
    let study = svc.create(&alice, db_id, "Notes", false).await.unwrap();
    let e4 = svc.add_move(&alice, study.id, 0, "e4").await.unwrap();

    svc.annotate(&alice, study.id, e4, Some("best by test".into()), Some(1))
        .await
        .unwrap();

    let tree: MoveTree =
        serde_json::from_str(&svc.get(&alice, study.id).await.unwrap().tree_json).unwrap();
    assert_eq!(tree.nodes[e4].comment.as_deref(), Some("best by test"));
    assert_eq!(tree.nodes[e4].nags, vec![1]);

    assert!(matches!(
        svc.annotate(&alice, study.id, 42, None, Some(2))
            .await
            .unwrap_err(),
        StudyError::InvalidNode(42)
    ));
}

#[tokio::test]
async fn set_shapes_pins_and_clears_a_node_plan() {
    let (svc, db_id) = setup().await;
    let alice = user("alice");
    let study = svc.create(&alice, db_id, "Plans", false).await.unwrap();
    let e4 = svc.add_move(&alice, study.id, 0, "e4").await.unwrap();

    let shapes = vec![Shape {
        orig: "g1".into(),
        dest: Some("f3".into()),
        brush: "green".into(),
    }];
    svc.set_shapes(&alice, study.id, e4, shapes.clone())
        .await
        .unwrap();
    assert_eq!(
        tree_of(&svc, &alice, study.id).await.nodes[e4].shapes,
        shapes
    );

    // An empty list clears the pin.
    svc.set_shapes(&alice, study.id, e4, Vec::new())
        .await
        .unwrap();
    assert!(tree_of(&svc, &alice, study.id).await.nodes[e4]
        .shapes
        .is_empty());

    // A bad node id is a 400-style InvalidNode, not a 500.
    assert!(matches!(
        svc.set_shapes(&alice, study.id, 99, Vec::new())
            .await
            .unwrap_err(),
        StudyError::InvalidNode(99)
    ));
}

#[tokio::test]
async fn promote_reorder_and_delete_restructure_the_tree() {
    let (svc, db_id) = setup().await;
    let alice = user("alice");
    let study = svc.create(&alice, db_id, "Lines", false).await.unwrap();

    // e4 with two replies: c5 (mainline) and e5 (variation).
    let e4 = svc.add_move(&alice, study.id, 0, "e4").await.unwrap();
    let _c5 = svc.add_move(&alice, study.id, e4, "c5").await.unwrap();
    let e5 = svc.add_move(&alice, study.id, e4, "e5").await.unwrap();

    // Promote the variation: e5 becomes the mainline reply.
    svc.promote_variation(&alice, study.id, e5).await.unwrap();
    assert_eq!(
        tree_of(&svc, &alice, study.id).await.mainline(),
        ["e4", "e5"]
    );

    // Reorder it back to second place: c5 is the mainline again.
    svc.reorder_variation(&alice, study.id, e5, 1)
        .await
        .unwrap();
    assert_eq!(
        tree_of(&svc, &alice, study.id).await.mainline(),
        ["e4", "c5"]
    );

    // Deleting e4 prunes the whole tree back to the root.
    svc.delete_node(&alice, study.id, e4).await.unwrap();
    assert!(tree_of(&svc, &alice, study.id).await.mainline().is_empty());

    // Structural edits on the root are rejected as bad edits, not 500s.
    assert!(matches!(
        svc.delete_node(&alice, study.id, 0).await.unwrap_err(),
        StudyError::InvalidEdit(_)
    ));
    assert!(matches!(
        svc.promote_variation(&alice, study.id, 99)
            .await
            .unwrap_err(),
        StudyError::InvalidNode(99)
    ));
}

#[tokio::test]
async fn cannot_mutate_another_users_study() {
    let (svc, db_id) = setup().await;
    let alice = user("alice");
    let bob = user("bob");

    let study = svc.create(&alice, db_id, "Private", false).await.unwrap();

    assert!(matches!(
        svc.add_move(&bob, study.id, 0, "e4").await.unwrap_err(),
        StudyError::Forbidden
    ));
    assert!(matches!(
        svc.annotate(&bob, study.id, 0, Some("x".into()), None)
            .await
            .unwrap_err(),
        StudyError::Forbidden
    ));
    assert!(matches!(
        svc.set_shapes(&bob, study.id, 0, Vec::new())
            .await
            .unwrap_err(),
        StudyError::Forbidden
    ));
    assert!(matches!(
        svc.promote_variation(&bob, study.id, 0).await.unwrap_err(),
        StudyError::Forbidden
    ));
    assert!(matches!(
        svc.reorder_variation(&bob, study.id, 0, 0)
            .await
            .unwrap_err(),
        StudyError::Forbidden
    ));
    assert!(matches!(
        svc.delete_node(&bob, study.id, 0).await.unwrap_err(),
        StudyError::Forbidden
    ));
    assert!(matches!(
        svc.delete(&bob, study.id).await.unwrap_err(),
        StudyError::Forbidden
    ));
    // Bob can't even see it.
    assert!(matches!(
        svc.get(&bob, study.id).await.unwrap_err(),
        StudyError::NotFound
    ));
    assert!(svc.list(&bob).await.unwrap().is_empty());
}

#[tokio::test]
async fn cannot_write_global_study_unless_admin() {
    let (svc, db_id) = setup().await;
    let alice = user("alice");
    let admin = CurrentUser::local_admin();

    // Only admin may create a global study.
    assert!(matches!(
        svc.create(&alice, db_id, "Global", true).await.unwrap_err(),
        StudyError::Forbidden
    ));
    let global = svc.create(&admin, db_id, "Global", true).await.unwrap();
    assert!(global.owner_id.is_none());

    // A non-admin can read it (global scope) but not write it.
    assert!(svc.get(&alice, global.id).await.is_ok());
    assert!(matches!(
        svc.add_move(&alice, global.id, 0, "e4").await.unwrap_err(),
        StudyError::Forbidden
    ));

    // Admin can.
    svc.add_move(&admin, global.id, 0, "e4").await.unwrap();
    let tree: MoveTree =
        serde_json::from_str(&svc.get(&admin, global.id).await.unwrap().tree_json).unwrap();
    assert_eq!(tree.mainline(), vec!["e4"]);
}

#[tokio::test]
async fn rename_updates_name_and_respects_write_guard() {
    let (svc, db_id) = setup().await;
    let alice = user("alice");
    let bob = user("bob");
    let study = svc.create(&alice, db_id, "Old name", false).await.unwrap();

    let renamed = svc.rename(&alice, study.id, "New name").await.unwrap();
    assert_eq!(renamed.name, "New name");
    assert_eq!(svc.get(&alice, study.id).await.unwrap().name, "New name");

    // Bob may not rename Alice's study; a missing study is a 404.
    assert!(matches!(
        svc.rename(&bob, study.id, "Hijacked").await.unwrap_err(),
        StudyError::Forbidden
    ));
    assert!(matches!(
        svc.rename(&alice, 999, "Ghost").await.unwrap_err(),
        StudyError::NotFound
    ));
}

#[tokio::test]
async fn pgn_import_export_round_trips() {
    let (svc, db_id) = setup().await;
    let alice = user("alice");
    let pgn = "1. e4 e5 (1... c5 2. Nf3) 2. Nf3 {develops} *";

    let study = svc
        .import_pgn(&alice, db_id, "Imported", pgn, false)
        .await
        .unwrap();
    let tree = tree_of(&svc, &alice, study.id).await;
    assert_eq!(tree.mainline(), vec!["e4", "e5", "Nf3"]);

    // Export, re-import, and confirm the mainline survives the round trip.
    let exported = svc.export_pgn(&alice, study.id).await.unwrap();
    let again = svc
        .import_pgn(&alice, db_id, "Reimported", &exported, false)
        .await
        .unwrap();
    assert_eq!(
        tree_of(&svc, &alice, again.id).await.mainline(),
        vec!["e4", "e5", "Nf3"]
    );
}

#[tokio::test]
async fn import_rejects_malformed_pgn_and_illegal_moves() {
    let (svc, db_id) = setup().await;
    let alice = user("alice");

    // An illegal move in otherwise well-formed PGN is a PGN error, not a 500.
    assert!(matches!(
        svc.import_pgn(&alice, db_id, "Bad", "1. e4 e4 *", false)
            .await
            .unwrap_err(),
        StudyError::Pgn(_)
    ));
    // Empty input has no game.
    assert!(matches!(
        svc.import_pgn(&alice, db_id, "Empty", "", false)
            .await
            .unwrap_err(),
        StudyError::Pgn(_)
    ));
}

#[tokio::test]
async fn list_scopes_to_own_plus_global() {
    let (svc, db_id) = setup().await;
    let alice = user("alice");
    let admin = CurrentUser::local_admin();

    svc.create(&alice, db_id, "Alice's", false).await.unwrap();
    svc.create(&admin, db_id, "Global", true).await.unwrap();

    // Alice sees her own study and the global one, but not Bob's would-be study.
    let names: Vec<_> = svc
        .list(&alice)
        .await
        .unwrap()
        .into_iter()
        .map(|s| s.name)
        .collect();
    assert_eq!(names, vec!["Alice's", "Global"]);
}
