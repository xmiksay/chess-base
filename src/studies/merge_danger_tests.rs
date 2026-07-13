//! Tests for the annotated danger-map merge (issue #177): pure comment/eval
//! formatting, plus service-level tests over an in-memory SQLite DB covering the
//! non-destructive contract (never touches a pre-existing node) and idempotency.

use super::*;
use crate::db::entities::databases;
use crate::db::{connect, DbConfig};
use crate::pgn_tree::Eval;
use crate::study_gen::{DangerKind, DangerNode, TrapVerdict};
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

const AFTER_E4: &str = "rnbqkbnr/pppppppp/8/8/4P3/8/PPPP1PPP/RNBQKBNR b KQkq e3 0 1";
const AFTER_E4_E5: &str = "rnbqkbnr/pppp1ppp/8/4p3/4P3/8/PPPP1PPP/RNBQKBNR w KQkq e6 0 2";
const AFTER_D4: &str = "rnbqkbnr/pppppppp/8/8/3P4/8/PPP1PPPP/RNBQKBNR b KQkq d3 0 1";

/// 0 (root) -1-> "e4" [Weapon trap, +0.30] -2-> "e5" [Caution baited trap, -0.40]
/// 0 (root) -3-> "d4" [Weapon only-move, +1.10, 42% miss rate]
fn sample_danger_tree() -> DangerTree {
    let node = |id, parent, san: Option<&str>, fen: &str, tag| DangerNode {
        id,
        parent,
        san: san.map(str::to_string),
        fen: fen.to_string(),
        ply: id,
        tag,
        children: match id {
            0 => vec![1, 3],
            1 => vec![2],
            _ => vec![],
        },
    };
    DangerTree {
        nodes: vec![
            node(0, None, None, crate::position::STARTPOS_FEN, None),
            node(
                1,
                Some(0),
                Some("e4"),
                AFTER_E4,
                Some(DangerTag {
                    kind: DangerKind::Trap,
                    role: DangerRole::Weapon,
                    trap: Some(TrapVerdict::Weapon),
                    only_move_gap: None,
                    miss_rate: None,
                    attack: None,
                    eval: Some(Eval::Cp(30)),
                }),
            ),
            node(
                2,
                Some(1),
                Some("e5"),
                AFTER_E4_E5,
                Some(DangerTag {
                    kind: DangerKind::Trap,
                    role: DangerRole::Caution,
                    trap: Some(TrapVerdict::HopeChess),
                    only_move_gap: None,
                    miss_rate: None,
                    attack: None,
                    eval: Some(Eval::Cp(-40)),
                }),
            ),
            node(
                3,
                Some(0),
                Some("d4"),
                AFTER_D4,
                Some(DangerTag {
                    kind: DangerKind::OnlyMove,
                    role: DangerRole::Weapon,
                    trap: None,
                    only_move_gap: Some(300),
                    miss_rate: Some(0.42),
                    attack: None,
                    eval: Some(Eval::Cp(110)),
                }),
            ),
        ],
        root: 0,
    }
}

fn node_by_san<'a>(tree: &'a MoveTree, san: &str) -> &'a crate::pgn_tree::Node {
    tree.nodes
        .iter()
        .find(|n| n.san.as_deref() == Some(san))
        .expect("node grafted")
}

#[tokio::test]
async fn grafts_and_annotates_every_newly_added_node() {
    let (svc, db_id) = setup().await;
    let alice = user("alice");
    let study = svc
        .create(&alice, db_id, "Repertoire", false)
        .await
        .unwrap();

    let outcome = svc
        .merge_danger(&alice, study.id, sample_danger_tree(), None)
        .await
        .unwrap();

    assert_eq!(outcome.added_nodes, 3);
    assert_eq!(outcome.weapons, 2);
    assert_eq!(outcome.cautions, 1);

    let tree: MoveTree = serde_json::from_str(&outcome.study.tree_json).unwrap();

    let e4 = node_by_san(&tree, "e4");
    assert_eq!(e4.eval, Some(Eval::Cp(30)));
    assert_eq!(
        e4.comment.as_deref(),
        Some("Weapon: trap, bounded downside on the best reply (+0.30)")
    );
    assert_eq!(e4.nags, vec![1]);

    let e5 = node_by_san(&tree, "e5");
    assert_eq!(e5.eval, Some(Eval::Cp(-40)));
    assert_eq!(
        e5.comment.as_deref(),
        Some("Caution: baited trap, the best reply refutes it (-0.40)")
    );
    assert_eq!(e5.nags, vec![6]);

    let d4 = node_by_san(&tree, "d4");
    assert_eq!(d4.eval, Some(Eval::Cp(110)));
    assert_eq!(
        d4.comment.as_deref(),
        Some("Weapon: only move, 42% miss rate (+1.10)")
    );
    assert_eq!(d4.nags, vec![1]);
}

#[tokio::test]
async fn never_touches_a_node_the_graft_only_followed() {
    let (svc, db_id) = setup().await;
    let alice = user("alice");
    let study = svc
        .create(&alice, db_id, "Repertoire", false)
        .await
        .unwrap();
    // Pre-existing prep on "e4" the graft must follow, not recreate.
    let e4_id = svc.add_move(&alice, study.id, 0, "e4").await.unwrap();
    svc.annotate(
        &alice,
        study.id,
        e4_id,
        Some("My own prep note".to_string()),
        Some(2), // "?"
    )
    .await
    .unwrap();

    let outcome = svc
        .merge_danger(&alice, study.id, sample_danger_tree(), None)
        .await
        .unwrap();

    // "e4" already existed: only "e5" (under it) and "d4" are new.
    assert_eq!(outcome.added_nodes, 2);

    let tree: MoveTree = serde_json::from_str(&outcome.study.tree_json).unwrap();
    let e4 = node_by_san(&tree, "e4");
    assert_eq!(e4.comment.as_deref(), Some("My own prep note"));
    assert_eq!(e4.nags, vec![2]);
    assert_eq!(
        e4.eval, None,
        "the walk's eval never overwrites a pre-existing node"
    );

    // The newly grafted child still gets annotated normally.
    let e5 = node_by_san(&tree, "e5");
    assert_eq!(e5.eval, Some(Eval::Cp(-40)));
}

#[tokio::test]
async fn re_merging_is_idempotent_and_reports_nothing_added() {
    let (svc, db_id) = setup().await;
    let alice = user("alice");
    let study = svc
        .create(&alice, db_id, "Repertoire", false)
        .await
        .unwrap();

    svc.merge_danger(&alice, study.id, sample_danger_tree(), None)
        .await
        .unwrap();
    let first = svc.get(&alice, study.id).await.unwrap().tree_json;

    let outcome = svc
        .merge_danger(&alice, study.id, sample_danger_tree(), None)
        .await
        .unwrap();
    let second = svc.get(&alice, study.id).await.unwrap().tree_json;

    assert_eq!(outcome.added_nodes, 0);
    assert_eq!(outcome.weapons, 0);
    assert_eq!(outcome.cautions, 0);
    assert_eq!(first, second);
}

#[tokio::test]
async fn ownership_and_existence_are_enforced() {
    let (svc, db_id) = setup().await;
    let alice = user("alice");
    let bob = user("bob");
    let study = svc
        .create(&alice, db_id, "Repertoire", false)
        .await
        .unwrap();

    assert!(matches!(
        svc.merge_danger(&bob, study.id, sample_danger_tree(), None)
            .await
            .unwrap_err(),
        StudyError::Forbidden
    ));
    assert!(matches!(
        svc.merge_danger(&alice, 9999, sample_danger_tree(), None)
            .await
            .unwrap_err(),
        StudyError::NotFound
    ));
}

#[test]
fn format_eval_signs_centipawns_and_prefixes_mate() {
    assert_eq!(format_eval(Eval::Cp(30)), "+0.30");
    assert_eq!(format_eval(Eval::Cp(-40)), "-0.40");
    assert_eq!(format_eval(Eval::Mate(3)), "M3");
    assert_eq!(format_eval(Eval::Mate(-2)), "-M2");
}

#[test]
fn danger_comment_covers_every_kind() {
    let tag = |kind, role, trap, only_move_gap, miss_rate, eval| DangerTag {
        kind,
        role,
        trap,
        only_move_gap,
        miss_rate,
        attack: None,
        eval,
    };

    assert_eq!(
        danger_comment(&tag(
            DangerKind::Trap,
            DangerRole::Weapon,
            Some(TrapVerdict::Weapon),
            None,
            None,
            Some(Eval::Cp(30))
        )),
        "Weapon: trap, bounded downside on the best reply (+0.30)"
    );
    assert_eq!(
        danger_comment(&tag(
            DangerKind::OnlyMove,
            DangerRole::Weapon,
            None,
            Some(250),
            Some(0.42),
            None
        )),
        "Weapon: only move, 42% miss rate"
    );
    assert_eq!(
        danger_comment(&tag(
            DangerKind::OffBook,
            DangerRole::OffBook,
            None,
            None,
            None,
            None
        )),
        "Off-book: no prepared answer in this repertoire"
    );
    // A gap can be `Some` on an Attack tag too (it's computed unconditionally) —
    // `danger_comment` must still switch on `kind`, not on which figures happen
    // to be populated.
    assert_eq!(
        danger_comment(&tag(
            DangerKind::Attack,
            DangerRole::Caution,
            None,
            Some(80),
            None,
            Some(Eval::Cp(-20))
        )),
        "Caution: pawn storm toward your king (-0.20)"
    );
}
