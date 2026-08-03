//! Service-level tests for the plan/threat shapes regenerate pass (issue #191)
//! over an in-memory SQLite DB: the Gate scenario (regenerate on, then off,
//! leaves zero generated-brush shapes while user shapes survive), the root
//! node participating, and ownership gating.

use super::*;
use crate::db::entities::databases;
use crate::db::{connect, DbConfig};
use crate::pgn_tree::Shape;
use crate::study_gen::plan_shapes::ShapeConfig;
use async_trait::async_trait;
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

/// A fake [`MultiAnalyzer`] returning one legal pawn-push PV for whichever side
/// is to move, so every visited node yields a plan arrow regardless of
/// position — exercises the regenerate pass without an engine process
/// (mirrors `study_gen::plan_shapes_tests::SideAwarePawn`).
struct SideAwarePawn;

#[async_trait]
impl MultiAnalyzer for SideAwarePawn {
    async fn analyse_multi(&self, fen: &str) -> anyhow::Result<Vec<crate::engine::Analysis>> {
        let white_to_move = fen.split(' ').nth(1) == Some("w");
        let uci = if white_to_move { "a2a3" } else { "a7a6" };
        Ok(vec![crate::engine::Analysis {
            bestmove: uci.to_string(),
            pv: vec![uci.to_string()],
            ..Default::default()
        }])
    }
}

fn user_shape() -> Shape {
    Shape {
        orig: "e2".into(),
        dest: Some("e4".into()),
        brush: "green".into(),
    }
}

#[tokio::test]
async fn pins_plan_arrows_on_every_node_including_the_root_and_keeps_user_shapes() {
    let (svc, db_id) = setup().await;
    let alice = user("alice");
    let study = svc.create(&alice, db_id, "Openings", false).await.unwrap();
    let e4 = svc.add_move(&alice, study.id, 0, "e4").await.unwrap();
    svc.set_shapes(&alice, study.id, e4, vec![user_shape()])
        .await
        .unwrap();

    let analyzer = SideAwarePawn;
    let cfg = ShapeConfig {
        plan_lines: 1,
        threats: false,
    };
    svc.regenerate_shapes(Some(&analyzer), &alice, study.id, &cfg)
        .await
        .unwrap();

    let tree = tree_of(&svc, &alice, study.id).await;
    assert!(
        tree.nodes[tree.root]
            .shapes
            .iter()
            .any(|s| s.brush == "plan1"),
        "root should carry a plan arrow too, not just move-bearing nodes"
    );
    assert!(tree.nodes[e4].shapes.iter().any(|s| s.brush == "plan1"));
    assert!(
        tree.nodes[e4].shapes.contains(&user_shape()),
        "the user-drawn shape must survive: {:?}",
        tree.nodes[e4].shapes
    );
}

#[tokio::test]
async fn re_regenerating_with_every_layer_off_strips_generated_arrows_but_keeps_user_shapes() {
    let (svc, db_id) = setup().await;
    let alice = user("alice");
    let study = svc.create(&alice, db_id, "Openings", false).await.unwrap();
    let e4 = svc.add_move(&alice, study.id, 0, "e4").await.unwrap();
    let _e5 = svc.add_move(&alice, study.id, e4, "e5").await.unwrap();
    svc.set_shapes(&alice, study.id, e4, vec![user_shape()])
        .await
        .unwrap();

    let analyzer = SideAwarePawn;
    let on = ShapeConfig {
        plan_lines: 3,
        threats: true,
    };
    svc.regenerate_shapes(Some(&analyzer), &alice, study.id, &on)
        .await
        .unwrap();
    // Sanity: the first pass actually pinned something to strip.
    let after_on = tree_of(&svc, &alice, study.id).await;
    assert!(after_on.nodes.iter().any(|n| !n.shapes.is_empty()));

    // Re-analysing with everything off, and no analyzer at all, must still
    // strip every generated arrow — this is the issue #191 Gate.
    let off = ShapeConfig::default();
    svc.regenerate_shapes(None, &alice, study.id, &off)
        .await
        .unwrap();

    let tree = tree_of(&svc, &alice, study.id).await;
    for node in &tree.nodes {
        assert!(
            node.shapes
                .iter()
                .all(|s| !["plan1", "plan2", "plan3", "threat"].contains(&s.brush.as_str())),
            "node {} still carries a generated shape: {:?}",
            node.id,
            node.shapes
        );
    }
    assert_eq!(
        tree.nodes[e4].shapes,
        vec![user_shape()],
        "the user-drawn shape must survive a strip pass"
    );
}

#[tokio::test]
async fn ownership_and_existence_are_enforced() {
    let (svc, db_id) = setup().await;
    let alice = user("alice");
    let bob = user("bob");
    let study = svc.create(&alice, db_id, "Openings", false).await.unwrap();

    let cfg = ShapeConfig::default();
    assert!(matches!(
        svc.regenerate_shapes(None, &bob, study.id, &cfg)
            .await
            .unwrap_err(),
        StudyError::Forbidden
    ));
    assert!(matches!(
        svc.regenerate_shapes(None, &alice, 9999, &cfg)
            .await
            .unwrap_err(),
        StudyError::NotFound
    ));
}

/// Deserialize a study's current move tree.
async fn tree_of(svc: &StudyService, user: &CurrentUser, id: i32) -> MoveTree {
    serde_json::from_str(&svc.get(user, id).await.unwrap().tree_json).unwrap()
}
