//! Tests for [`super`] (study move-tree generation). Split out to keep the
//! module under the project's 500-line file cap.

use super::*;
use crate::position::{replay, STARTPOS_FEN};
use std::collections::HashMap;

fn cand(san: &str, frequency: f64, eval_cp: i32) -> Candidate {
    Candidate {
        san: san.to_string(),
        frequency,
        eval_cp,
    }
}

fn config(min_frequency: f64, eval_margin_cp: i32, max_children: usize) -> TreeConfig {
    TreeConfig {
        max_depth: 10,
        max_children,
        max_nodes: 1000,
        min_frequency,
        eval_margin_cp,
    }
}

#[test]
fn score_to_cp_passes_centipawns_and_folds_mate() {
    assert_eq!(score_to_cp(Some(Score::Cp { value: 35 })), 35);
    assert_eq!(score_to_cp(None), 0);
    // Mate in 1 outranks mate in 5; both beat any centipawn eval.
    let m1 = score_to_cp(Some(Score::Mate { value: 1 }));
    let m5 = score_to_cp(Some(Score::Mate { value: 5 }));
    assert!(m1 > m5);
    assert!(m5 > score_to_cp(Some(Score::Cp { value: 5000 })));
    // Being mated is symmetric and negative.
    assert_eq!(score_to_cp(Some(Score::Mate { value: -1 })), -m1);
}

#[test]
fn pruning_drops_below_frequency_floor() {
    let cands = [
        cand("e4", 0.6, 20),
        cand("d4", 0.3, 20),
        cand("a3", 0.02, 20),
    ];
    let keep = select_continuations(&cands, &config(0.05, 1000, 10));
    let kept: Vec<&str> = keep.iter().map(|&i| cands[i].san.as_str()).collect();
    assert_eq!(kept, vec!["e4", "d4"]); // a3 is too rare
}

#[test]
fn pruning_drops_moves_outside_the_eval_margin() {
    // Best is +50; with a 30cp margin, anything below +20 is cut.
    let cands = [
        cand("e4", 0.4, 50),
        cand("c4", 0.4, 25),
        cand("b3", 0.4, -100),
    ];
    let keep = select_continuations(&cands, &config(0.0, 30, 10));
    let kept: Vec<&str> = keep.iter().map(|&i| cands[i].san.as_str()).collect();
    assert_eq!(kept, vec!["e4", "c4"]); // b3 is too far behind
}

#[test]
fn pruning_caps_breadth_keeping_most_frequent() {
    let cands = [
        cand("e4", 0.5, 0),
        cand("d4", 0.3, 0),
        cand("c4", 0.15, 0),
        cand("Nf3", 0.05, 0),
    ];
    let keep = select_continuations(&cands, &config(0.0, 1000, 2));
    let kept: Vec<&str> = keep.iter().map(|&i| cands[i].san.as_str()).collect();
    assert_eq!(kept, vec!["e4", "d4"]); // top two by frequency
}

#[test]
fn pruning_is_deterministic_with_stable_tiebreaks() {
    // Equal frequency and eval ⇒ SAN ascending decides the order.
    let cands = [
        cand("d4", 0.5, 10),
        cand("c4", 0.5, 10),
        cand("e4", 0.5, 10),
    ];
    let keep = select_continuations(&cands, &config(0.0, 1000, 10));
    let kept: Vec<&str> = keep.iter().map(|&i| cands[i].san.as_str()).collect();
    assert_eq!(kept, vec!["c4", "d4", "e4"]);
}

#[test]
fn pruning_returns_empty_when_all_below_floor() {
    let cands = [cand("a3", 0.01, 0), cand("h3", 0.01, 0)];
    assert!(select_continuations(&cands, &config(0.5, 1000, 10)).is_empty());
}

// --- builder, against deterministic fakes -------------------------------

/// Continuations keyed by FEN; positions absent from the map are leaves.
struct FakeStats(HashMap<String, Vec<MoveReport>>);
/// Evals keyed by FEN; positions absent return `None`.
struct FakeEval(HashMap<String, Score>);

#[async_trait]
impl ContinuationSource for FakeStats {
    async fn continuations(&self, fen: &str) -> Result<Vec<MoveReport>> {
        Ok(self.0.get(fen).cloned().unwrap_or_default())
    }
}

#[async_trait]
impl Evaluator for FakeEval {
    async fn eval(&self, fen: &str) -> Result<Option<Score>> {
        Ok(self.0.get(fen).copied())
    }
}

fn fen_after(sans: &[&str]) -> String {
    if sans.is_empty() {
        return STARTPOS_FEN.to_string();
    }
    replay(STARTPOS_FEN, sans, STD)
        .unwrap()
        .last()
        .unwrap()
        .fen
        .clone()
}

fn report(san: &str, frequency: f64) -> MoveReport {
    MoveReport {
        san: san.to_string(),
        count: (frequency * 100.0) as u64,
        white: 0,
        draws: 0,
        black: 0,
        frequency,
        score: 0.5,
    }
}

/// A small fixture: from the start, e4 (common) and d4 (common); after e4,
/// only c5. Evals make e4 the engine's pick over d4.
fn fixture() -> (FakeEval, FakeStats) {
    let mut conts = HashMap::new();
    conts.insert(
        fen_after(&[]),
        vec![report("e4", 0.6), report("d4", 0.3), report("a3", 0.02)],
    );
    conts.insert(fen_after(&["e4"]), vec![report("c5", 0.7)]);
    let stats = FakeStats(conts);

    let mut evals = HashMap::new();
    // Child evals are from the side-to-move *after* the move (Black/White).
    evals.insert(fen_after(&[]), Score::Cp { value: 20 });
    evals.insert(fen_after(&["e4"]), Score::Cp { value: -30 }); // Black slightly worse ⇒ good for White
    evals.insert(fen_after(&["d4"]), Score::Cp { value: 40 }); // Black better ⇒ worse for White
    evals.insert(fen_after(&["e4", "c5"]), Score::Cp { value: 25 });
    (FakeEval(evals), FakeStats(stats.0))
}

#[tokio::test]
async fn builds_a_tagged_tree_with_eval_and_stats() {
    let (eval, stats) = fixture();
    let cfg = TreeConfig {
        max_depth: 2,
        max_children: 5,
        max_nodes: 1000,
        min_frequency: 0.05,
        eval_margin_cp: 1000, // wide ⇒ eval doesn't prune here
    };
    let tree = build_tree(&eval, &stats, &fen_after(&[]), &cfg)
        .await
        .unwrap();

    let root = &tree.nodes[tree.root];
    assert_eq!(root.san, None);
    assert_eq!(root.stats, None);
    assert_eq!(root.eval, Some(Score::Cp { value: 20 }));
    // e4 and d4 kept (a3 below the frequency floor), e4 first (more frequent).
    let root_moves: Vec<&str> = root
        .children
        .iter()
        .map(|&c| tree.nodes[c].san.as_deref().unwrap())
        .collect();
    assert_eq!(root_moves, vec!["e4", "d4"]);

    // The e4 node carries its DB stats and the engine eval of the position.
    let e4 = &tree.nodes[root.children[0]];
    assert_eq!(e4.stats.as_ref().unwrap().san, "e4");
    assert_eq!(e4.eval, Some(Score::Cp { value: -30 }));
    assert_eq!(e4.ply, 1);
    // It expands one more ply to c5; d4 is a leaf (no continuations mapped).
    assert_eq!(tree.nodes[e4.children[0]].san.as_deref(), Some("c5"));
    assert!(tree.nodes[root.children[1]].children.is_empty());
}

#[tokio::test]
async fn respects_max_depth() {
    let (eval, stats) = fixture();
    let cfg = TreeConfig {
        max_depth: 1,
        ..TreeConfig::default()
    };
    let tree = build_tree(&eval, &stats, &fen_after(&[]), &cfg)
        .await
        .unwrap();
    // Depth 1: root's children exist but none of them expand further.
    assert!(tree.nodes.iter().all(|n| n.ply <= 1));
    for node in &tree.nodes {
        if node.ply == 1 {
            assert!(node.children.is_empty());
        }
    }
}

#[tokio::test]
async fn eval_margin_prunes_the_weaker_first_move() {
    let (eval, stats) = fixture();
    // From White's view e4 scores +30 (child -30 negated), d4 scores -40.
    // A 50cp margin cuts d4.
    let cfg = TreeConfig {
        max_depth: 1,
        max_children: 5,
        max_nodes: 1000,
        min_frequency: 0.05,
        eval_margin_cp: 50,
    };
    let tree = build_tree(&eval, &stats, &fen_after(&[]), &cfg)
        .await
        .unwrap();
    let root = &tree.nodes[tree.root];
    let moves: Vec<&str> = root
        .children
        .iter()
        .map(|&c| tree.nodes[c].san.as_deref().unwrap())
        .collect();
    assert_eq!(moves, vec!["e4"]);
}

#[tokio::test]
async fn respects_global_node_budget() {
    let (eval, stats) = fixture();
    let cfg = TreeConfig {
        max_depth: 5,
        max_children: 5,
        max_nodes: 2, // root + one child only
        min_frequency: 0.05,
        eval_margin_cp: 1000,
    };
    let tree = build_tree(&eval, &stats, &fen_after(&[]), &cfg)
        .await
        .unwrap();
    assert_eq!(tree.nodes.len(), 2);
}

#[tokio::test]
async fn rejects_an_invalid_fen() {
    let (eval, stats) = fixture();
    let err = build_tree(&eval, &stats, "not a fen", &TreeConfig::default())
        .await
        .unwrap_err();
    assert!(matches!(err, TreeError::InvalidFen(_)));
}

#[tokio::test]
async fn output_round_trips_through_json() {
    let (eval, stats) = fixture();
    let tree = build_tree(&eval, &stats, &fen_after(&[]), &TreeConfig::default())
        .await
        .unwrap();
    let json = serde_json::to_string(&tree).unwrap();
    let back: VariationTree = serde_json::from_str(&json).unwrap();
    assert_eq!(tree, back);
}
