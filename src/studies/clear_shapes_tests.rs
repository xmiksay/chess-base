//! Service-level tests for the bulk shape-clear pass (issue #191) over an
//! in-memory SQLite DB: `generated` keeps user shapes, `all` clears everything,
//! and ownership/existence are enforced like every other write.

use super::*;
use crate::db::entities::databases;
use crate::db::{connect, DbConfig};
use crate::pgn_tree::Shape;
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

fn plan_shape() -> Shape {
    Shape {
        orig: "g1".into(),
        dest: Some("f3".into()),
        brush: "plan1".into(),
    }
}

fn user_shape() -> Shape {
    Shape {
        orig: "e2".into(),
        dest: Some("e4".into()),
        brush: "green".into(),
    }
}

async fn tree_of(svc: &StudyService, user: &CurrentUser, id: i32) -> MoveTree {
    serde_json::from_str(&svc.get(user, id).await.unwrap().tree_json).unwrap()
}

#[tokio::test]
async fn generated_scope_strips_generated_brushes_and_keeps_user_shapes() {
    let (svc, db_id) = setup().await;
    let alice = user("alice");
    let study = svc.create(&alice, db_id, "Openings", false).await.unwrap();
    let e4 = svc.add_move(&alice, study.id, 0, "e4").await.unwrap();
    svc.set_shapes(&alice, study.id, e4, vec![plan_shape(), user_shape()])
        .await
        .unwrap();

    svc.clear_shapes(&alice, study.id, ClearShapesScope::Generated)
        .await
        .unwrap();

    let tree = tree_of(&svc, &alice, study.id).await;
    assert_eq!(tree.nodes[e4].shapes, vec![user_shape()]);
}

#[tokio::test]
async fn all_scope_clears_every_shape() {
    let (svc, db_id) = setup().await;
    let alice = user("alice");
    let study = svc.create(&alice, db_id, "Openings", false).await.unwrap();
    let e4 = svc.add_move(&alice, study.id, 0, "e4").await.unwrap();
    svc.set_shapes(&alice, study.id, e4, vec![plan_shape(), user_shape()])
        .await
        .unwrap();

    svc.clear_shapes(&alice, study.id, ClearShapesScope::All)
        .await
        .unwrap();

    let tree = tree_of(&svc, &alice, study.id).await;
    assert!(tree.nodes[e4].shapes.is_empty());
}

#[tokio::test]
async fn ownership_and_existence_are_enforced() {
    let (svc, db_id) = setup().await;
    let alice = user("alice");
    let bob = user("bob");
    let study = svc.create(&alice, db_id, "Openings", false).await.unwrap();

    assert!(matches!(
        svc.clear_shapes(&bob, study.id, ClearShapesScope::All)
            .await
            .unwrap_err(),
        StudyError::Forbidden
    ));
    assert!(matches!(
        svc.clear_shapes(&alice, 9999, ClearShapesScope::All)
            .await
            .unwrap_err(),
        StudyError::NotFound
    ));
}
