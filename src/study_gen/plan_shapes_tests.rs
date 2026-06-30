//! Unit tests for the plan/threat shapes pass. Pure helpers are checked directly;
//! [`apply_shapes`] runs against a fake [`MultiAnalyzer`] so no engine is needed.

use super::*;
use crate::plans::{Plan, Trajectory};
use crate::position::STARTPOS_FEN;
use crate::study_gen::tree::{VariationNode, VariationTree};
use async_trait::async_trait;

const STD: CastlingMode = CastlingMode::Standard;

fn traj(piece: char, squares: &[&str]) -> Trajectory {
    Trajectory {
        piece,
        squares: squares.iter().map(|s| s.to_string()).collect(),
    }
}

fn pv(moves: &[&str]) -> Vec<String> {
    moves.iter().map(|s| s.to_string()).collect()
}

/// A fake analyser returning one PV that is legal for whichever side is to move
/// (a trivial a-pawn push), so every visited node yields a plan regardless of
/// side — exercising apply_shapes' per-node walk without an engine.
struct SideAwarePawn;

#[async_trait]
impl MultiAnalyzer for SideAwarePawn {
    async fn analyse_multi(&self, fen: &str) -> anyhow::Result<Vec<crate::engine::Analysis>> {
        let white_to_move = fen.split(' ').nth(1) == Some("w");
        let uci = if white_to_move { "a2a3" } else { "a7a6" };
        Ok(vec![crate::engine::Analysis {
            bestmove: uci.to_string(),
            ponder: None,
            score: None,
            depth: None,
            pv: vec![uci.to_string()],
        }])
    }
}

#[test]
fn plan_to_shapes_makes_an_arrow_per_consecutive_pair() {
    let plan = Plan {
        trajectories: vec![traj('N', &["g1", "f3", "g5"])],
    };
    let shapes = plan_to_shapes(&plan, "plan1");
    assert_eq!(shapes.len(), 2);
    assert_eq!(shapes[0].orig, "g1");
    assert_eq!(shapes[0].dest.as_deref(), Some("f3"));
    assert_eq!(shapes[1].orig, "f3");
    assert_eq!(shapes[1].dest.as_deref(), Some("g5"));
    assert!(shapes.iter().all(|s| s.brush == "plan1"));
}

#[test]
fn plan_to_shapes_skips_zero_length_segments() {
    // A degenerate trajectory that repeats a square contributes no arrow.
    let plan = Plan {
        trajectories: vec![traj('P', &["e2", "e2"])],
    };
    assert!(plan_to_shapes(&plan, "plan1").is_empty());
}

#[test]
fn node_shapes_brushes_each_line_by_rank_and_caps_at_three() {
    // Four lines requested, but only plan1..plan3 brushes exist → 4th dropped.
    let pvs = vec![pv(&["g1f3"]), pv(&["e2e4"]), pv(&["d2d4"]), pv(&["c2c4"])];
    let shapes = node_shapes(STARTPOS_FEN, &pvs, 4, false, STD);
    let brushes: std::collections::BTreeSet<_> = shapes.iter().map(|s| s.brush.clone()).collect();
    assert_eq!(
        brushes,
        ["plan1", "plan2", "plan3"]
            .iter()
            .map(|s| s.to_string())
            .collect()
    );
    assert!(!shapes.iter().any(|s| s.brush == "plan4"));
}

#[test]
fn node_shapes_appends_threats_when_requested() {
    // White to move with its own queen hanging on d5 (attacked by the black e6
    // pawn, undefended): the threats scan should emit a `threat` arrow.
    let fen = "rnbqkbnr/pppp1ppp/4p3/3Q4/8/8/PPPPPPPP/RNB1KBNR w KQkq - 0 1";
    let shapes = node_shapes(fen, &[], 0, true, STD);
    assert!(
        shapes.iter().any(|s| s.brush == "threat"),
        "expected a threat arrow for the hanging queen, got {shapes:?}"
    );
}

#[test]
fn node_shapes_off_is_empty() {
    let shapes = node_shapes(STARTPOS_FEN, &[pv(&["e2e4"])], 0, false, STD);
    assert!(shapes.is_empty());
}

#[tokio::test]
async fn apply_shapes_tags_every_node_with_plans() {
    let mut tree = two_node_tree();
    let analyzer = SideAwarePawn;
    let cfg = ShapeConfig {
        plan_lines: 1,
        threats: false,
    };
    apply_shapes(Some(&analyzer), &mut tree, &cfg, STD).await;

    assert!(
        tree.nodes.iter().all(|n| !n.shapes.is_empty()),
        "every node should carry plan arrows"
    );
    assert!(tree
        .nodes
        .iter()
        .all(|n| n.shapes.iter().all(|s| s.brush == "plan1")));
}

#[tokio::test]
async fn apply_shapes_threats_only_needs_no_analyzer() {
    let mut tree = single_node_tree("rnbqkbnr/pppp1ppp/4p3/3Q4/8/8/PPPPPPPP/RNB1KBNR w KQkq - 0 1");
    let cfg = ShapeConfig {
        plan_lines: 0,
        threats: true,
    };
    apply_shapes(None, &mut tree, &cfg, STD).await;
    assert!(tree.nodes[0].shapes.iter().any(|s| s.brush == "threat"));
}

#[tokio::test]
async fn apply_shapes_off_is_a_noop() {
    let mut tree = single_node_tree(STARTPOS_FEN);
    apply_shapes(None, &mut tree, &ShapeConfig::default(), STD).await;
    assert!(tree.nodes[0].shapes.is_empty());
}

// --- tiny tree builders ---------------------------------------------------

fn single_node_tree(fen: &str) -> VariationTree {
    VariationTree {
        nodes: vec![bare_node(0, None, None, fen)],
        root: 0,
    }
}

fn two_node_tree() -> VariationTree {
    let mut root = bare_node(0, None, None, STARTPOS_FEN);
    root.children = vec![1];
    let child = bare_node(
        1,
        Some(0),
        Some("e4"),
        "rnbqkbnr/pppppppp/8/8/4P3/8/PPPP1PPP/RNBQKBNR b KQkq - 0 1",
    );
    VariationTree {
        nodes: vec![root, child],
        root: 0,
    }
}

fn bare_node(id: usize, parent: Option<usize>, san: Option<&str>, fen: &str) -> VariationNode {
    VariationNode {
        id,
        parent,
        san: san.map(Into::into),
        fen: fen.into(),
        zobrist: "0000000000000000".into(),
        ply: parent.map(|_| 1).unwrap_or(0),
        eval: None,
        stats: None,
        eco: None,
        concepts: Default::default(),
        shapes: Vec::new(),
        children: Vec::new(),
    }
}
