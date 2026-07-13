//! Unit tests for the danger-map route DTOs: the request body defaults and the
//! partial `SpineConfig` override semantics (issue #141). The full happy path
//! needs a real engine + LLM, so it is covered at the service layer with injected
//! fakes (`study_gen::danger_generate` tests) and the mounted-route 503 path lives
//! in `tests/studies.rs`.

use super::*;
use crate::study_gen::spine::Side;

#[test]
fn body_defaults_when_only_required_fields_are_given() {
    let body: DangerMapBody = serde_json::from_value(serde_json::json!({
        "database_id": 7,
        "name": "Najdorf traps",
        "spine_pgn": "1. e4 c5 *",
    }))
    .expect("minimal body deserializes");

    assert_eq!(body.database_id, 7);
    assert!(!body.global);
    assert!(body.start_fen.is_none());
    assert!(body.model.is_none());
    assert!(body.movetime_ms.is_none());
    assert!(body.multipv.is_none());
    // An omitted `spine` falls back to the full defaults.
    assert_eq!(body.spine, SpineConfig::default());
}

#[test]
fn spine_accepts_partial_overrides_keeping_other_defaults() {
    let body: DangerMapBody = serde_json::from_value(serde_json::json!({
        "database_id": 1,
        "name": "Black weapons",
        "spine_pgn": "1. e4 e5 *",
        "spine": {
            "our_side": "Black",
            "max_depth": 12,
            "danger": { "only_move_gap_cp": 200 }
        }
    }))
    .expect("partial spine overrides deserialize");

    let defaults = SpineConfig::default();
    // Overridden fields take the request value...
    assert_eq!(body.spine.our_side, Side::Black);
    assert_eq!(body.spine.max_depth, 12);
    assert_eq!(body.spine.danger.only_move_gap_cp, 200);
    // ...while everything else (including the rest of `danger`) keeps defaults.
    assert_eq!(body.spine.min_frequency, defaults.min_frequency);
    assert_eq!(body.spine.max_replies, defaults.max_replies);
    assert_eq!(
        body.spine.danger.downside_floor_cp,
        defaults.danger.downside_floor_cp
    );
    assert_eq!(body.spine.attack, defaults.attack);
}

#[test]
fn danger_walk_body_needs_only_a_spine_and_defaults_the_rest() {
    // The engine-only `/api/studies/danger-map` body (issue #156) carries no
    // database/name — it returns data, not a persisted study.
    let body: DangerWalkBody = serde_json::from_value(serde_json::json!({
        "spine_pgn": "1. e4 c5 *",
    }))
    .expect("minimal danger-walk body deserializes");

    assert_eq!(body.spine_pgn, "1. e4 c5 *");
    assert!(body.fen.is_none());
    assert!(body.movetime_ms.is_none());
    assert!(body.multipv.is_none());
    assert_eq!(body.spine, SpineConfig::default());
}

#[test]
fn danger_walk_body_accepts_partial_spine_overrides() {
    let body: DangerWalkBody = serde_json::from_value(serde_json::json!({
        "spine_pgn": "1. e4 e5 *",
        "fen": "rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w KQkq - 0 1",
        "movetime_ms": 250,
        "multipv": 3,
        "spine": { "our_side": "Black", "max_depth": 10 }
    }))
    .expect("danger-walk body with overrides deserializes");

    let defaults = SpineConfig::default();
    assert_eq!(body.movetime_ms, Some(250));
    assert_eq!(body.multipv, Some(3));
    assert_eq!(body.spine.our_side, Side::Black);
    assert_eq!(body.spine.max_depth, 10);
    // Untouched fields keep their defaults.
    assert_eq!(body.spine.min_frequency, defaults.min_frequency);
}

#[test]
fn roles_digest_keeps_only_tagged_nodes_in_walk_order() {
    use crate::pgn_tree::Eval;
    use crate::study_gen::{DangerKind, DangerNode, DangerRole, DangerTag, DangerTree};

    let tag = |kind, role, eval| DangerTag {
        kind,
        role,
        trap: None,
        only_move_gap: None,
        miss_rate: None,
        attack: None,
        eval,
    };
    let node = |id: usize, san: Option<&str>, tag: Option<DangerTag>| DangerNode {
        id,
        parent: if id == 0 { None } else { Some(id - 1) },
        san: san.map(str::to_string),
        fen: STARTPOS_FEN.to_string(),
        ply: id,
        tag,
        children: vec![],
    };

    let tree = DangerTree {
        nodes: vec![
            node(0, None, None),       // root, untagged
            node(1, Some("e4"), None), // plain spine move
            node(
                2,
                Some("Qh5"),
                Some(tag(
                    DangerKind::Trap,
                    DangerRole::Caution,
                    Some(Eval::Cp(-40)),
                )),
            ),
            node(
                3,
                Some("Nf6"),
                Some(tag(DangerKind::OnlyMove, DangerRole::Weapon, None)),
            ),
        ],
        root: 0,
    };

    let roles = roles_digest(&tree);
    assert_eq!(roles.len(), 2);
    assert_eq!(roles[0].node_id, 2);
    assert_eq!(roles[0].san.as_deref(), Some("Qh5"));
    assert_eq!(roles[0].kind, "Trap");
    assert_eq!(roles[0].role, "Caution");
    assert_eq!(roles[0].eval, Some(Eval::Cp(-40)));
    assert_eq!(roles[1].role, "Weapon");
    assert_eq!(roles[1].eval, None);
}
