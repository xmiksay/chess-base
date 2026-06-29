//! Variation tree builder (issue #29): a deterministic preprocessing **stage**,
//! not an LLM tool. From a start FEN it walks the DB-played continuations
//! breadth-first, tags every node with an engine evaluation and the pre-chewed
//! DB statistics (issue #28), and prunes by frequency + eval thresholds down to
//! a bounded, *teachable* size. The LLM never expands the tree move-by-move; it
//! receives the finished [`VariationTree`] as plain serializable data (ADR-0009:
//! the engine/DB are ground truth, the model only annotates).
//!
//! Layered for testability: the tree types, the [`select_continuations`] pruning
//! predicate and the generic [`build_tree`] walk are I/O-free — they depend only
//! on the two seams [`Evaluator`] and [`ContinuationSource`], so the whole stage
//! is unit-tested against fakes. The concrete engine/DB adapters live in the
//! parent module ([`super`]).

use std::cmp::Ordering;
use std::collections::VecDeque;

use anyhow::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::engine::Score;
use crate::openings::opening_of_zobrist;
use crate::position::{apply_san, zobrist_of_fen, CastlingMode};
use crate::search::report::{EcoInfo, MoveReport};

const STD: CastlingMode = CastlingMode::Standard;

/// Centipawn magnitude a forced mate maps onto, kept well clear of any real
/// centipawn eval so mate always dominates and `mate in 1` beats `mate in 5`.
const MATE_CP: i32 = 1_000_000;

/// Engine evaluation seam: the eval of a position from *its* side-to-move's
/// perspective. The variation builder asks this for every position it visits;
/// the real implementation routes through the pooled engine facade.
#[async_trait]
pub trait Evaluator {
    async fn eval(&self, fen: &str) -> Result<Option<Score>>;
}

/// DB-statistics seam: the continuations played from a position with their
/// win/draw/loss, frequency and score (the pre-chewed DB layer, issue #28). The
/// caller scope is bound into the implementation, so the builder stays identity-
/// agnostic.
#[async_trait]
pub trait ContinuationSource {
    async fn continuations(&self, fen: &str) -> Result<Vec<MoveReport>>;
}

/// How big a tree to build and how hard to prune it. Pure tree shape — engine
/// search limits live on the evaluator, not here. `serde` so a study request can
/// carry one over the wire.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TreeConfig {
    /// Maximum plies from the root (root is ply 0). Caps tree depth.
    pub max_depth: usize,
    /// Maximum continuations kept at any node. Caps tree breadth.
    pub max_children: usize,
    /// Global cap on total nodes (root included). A safety bound: breadth-first
    /// expansion keeps the shallowest, most frequent lines when it bites.
    pub max_nodes: usize,
    /// Drop continuations played in a smaller share of games than this (`0..=1`).
    pub min_frequency: f64,
    /// Keep continuations within this many centipawns of the best surviving
    /// sibling (from the moving side's perspective); drop those clearly worse.
    pub eval_margin_cp: i32,
}

impl Default for TreeConfig {
    fn default() -> Self {
        Self {
            max_depth: 6,
            max_children: 3,
            max_nodes: 64,
            min_frequency: 0.05,
            eval_margin_cp: 100,
        }
    }
}

/// Why building a variation tree failed. Transport-agnostic.
#[derive(Debug, thiserror::Error)]
pub enum TreeError {
    #[error("invalid FEN: {0}")]
    InvalidFen(String),
    /// An evaluator or continuation-source failure, propagated verbatim.
    #[error(transparent)]
    Source(#[from] anyhow::Error),
}

/// One tagged node of the variation tree. Arena-allocated (`id` indexes
/// [`VariationTree::nodes`]; `children` holds child ids) mirroring
/// [`crate::pgn_tree::MoveTree`], so it serializes the same familiar way.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VariationNode {
    pub id: usize,
    pub parent: Option<usize>,
    /// SAN of the move leading to this node; `None` only at the root.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub san: Option<String>,
    pub fen: String,
    /// Zobrist key as zero-padded hex (matches the DB report; dodges JSON's
    /// 64-bit precision loss).
    pub zobrist: String,
    /// Plies from the root.
    pub ply: usize,
    /// Engine evaluation of this position, from its side-to-move's perspective.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub eval: Option<Score>,
    /// DB statistics for the move leading here (frequency/score/W-D-L). `None` at
    /// the root, which no move reaches.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stats: Option<MoveReport>,
    /// ECO classification of this position, if the opening is known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub eco: Option<EcoInfo>,
    pub children: Vec<usize>,
}

/// A bounded, pruned, tagged variation tree — the finished output of the stage.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VariationTree {
    pub nodes: Vec<VariationNode>,
    pub root: usize,
}

/// A continuation under consideration during pruning. `eval_cp` is the eval
/// **from the moving side's perspective** in centipawn-equivalent units (mate
/// folded onto [`MATE_CP`]), so larger is always better for the side to move.
#[derive(Debug, Clone, PartialEq)]
pub struct Candidate {
    pub san: String,
    pub frequency: f64,
    pub eval_cp: i32,
}

/// Fold an engine [`Score`] into a single centipawn-equivalent scalar (larger is
/// better for the scored side). A missing score maps to `0` (neutral) so an
/// un-scored position never silently wins or loses the eval comparison. Mate is
/// mapped onto [`MATE_CP`] minus the distance, so `mate in 1` outranks `mate in
/// 5` and being mated is symmetrically negative.
pub fn score_to_cp(score: Option<Score>) -> i32 {
    match score {
        None => 0,
        Some(Score::Cp { value }) => value.clamp(-(MATE_CP - 1), MATE_CP - 1),
        Some(Score::Mate { value }) => {
            let dist = value.unsigned_abs().min((MATE_CP - 1) as u32) as i32;
            if value >= 0 {
                MATE_CP - dist
            } else {
                -(MATE_CP - dist)
            }
        }
    }
}

/// Pure pruning: from `candidates` keep those that clear the frequency floor and
/// fall within `eval_margin_cp` of the best surviving eval, then the most
/// frequent `max_children`. Returns kept indices in output order — frequency
/// desc, then eval desc, then SAN asc — so the result is fully deterministic.
pub fn select_continuations(candidates: &[Candidate], config: &TreeConfig) -> Vec<usize> {
    let mut kept: Vec<usize> = candidates
        .iter()
        .enumerate()
        .filter(|(_, c)| c.frequency >= config.min_frequency)
        .map(|(i, _)| i)
        .collect();

    let best = match kept.iter().map(|&i| candidates[i].eval_cp).max() {
        Some(best) => best,
        None => return kept, // empty after the frequency floor
    };
    let cutoff = best.saturating_sub(config.eval_margin_cp);
    kept.retain(|&i| candidates[i].eval_cp >= cutoff);

    kept.sort_by(|&a, &b| {
        let (ca, cb) = (&candidates[a], &candidates[b]);
        cb.frequency
            .partial_cmp(&ca.frequency)
            .unwrap_or(Ordering::Equal)
            .then(cb.eval_cp.cmp(&ca.eval_cp))
            .then(ca.san.cmp(&cb.san))
    });
    kept.truncate(config.max_children);
    kept
}

/// ECO classification for a Zobrist key, shaped as the DB report's [`EcoInfo`].
fn eco_for(zobrist: u64) -> Option<EcoInfo> {
    opening_of_zobrist(zobrist).map(|o| EcoInfo {
        eco: o.eco.to_string(),
        name: o.name.to_string(),
    })
}

/// Build a bounded, pruned, tagged variation tree from `start_fen`.
///
/// Breadth-first: each visited position's DB continuations are filtered by
/// frequency, the survivors evaluated, then [`select_continuations`] picks the
/// kept moves. Expansion stops at `max_depth`, `max_children` per node, and the
/// global `max_nodes` budget. Deterministic for deterministic seams.
pub async fn build_tree<E, S>(
    eval: &E,
    stats: &S,
    start_fen: &str,
    config: &TreeConfig,
) -> Result<VariationTree, TreeError>
where
    E: Evaluator + Sync,
    S: ContinuationSource + Sync,
{
    let root_zobrist =
        zobrist_of_fen(start_fen, STD).map_err(|e| TreeError::InvalidFen(e.to_string()))?;
    let root_eval = eval.eval(start_fen).await?;

    let mut nodes = vec![VariationNode {
        id: 0,
        parent: None,
        san: None,
        fen: start_fen.to_string(),
        zobrist: format!("{root_zobrist:016x}"),
        ply: 0,
        eval: root_eval,
        stats: None,
        eco: eco_for(root_zobrist),
        children: Vec::new(),
    }];

    let mut queue = VecDeque::from([0usize]);
    while let Some(idx) = queue.pop_front() {
        let ply = nodes[idx].ply;
        if ply >= config.max_depth || nodes.len() >= config.max_nodes {
            continue;
        }
        let fen = nodes[idx].fen.clone();

        // Pre-filter on frequency before spending an engine eval per move, then
        // evaluate each survivor's resulting position.
        let mut scored: Vec<(MoveReport, String, u64, Option<Score>)> = Vec::new();
        for mv in stats.continuations(&fen).await? {
            if mv.frequency < config.min_frequency {
                continue;
            }
            let (child_fen, child_zobrist) = match apply_san(&fen, &mv.san, STD) {
                Ok(pair) => pair,
                Err(_) => continue, // a move the DB has that no longer parses — skip
            };
            let child_eval = eval.eval(&child_fen).await?;
            scored.push((mv, child_fen, child_zobrist, child_eval));
        }

        let candidates: Vec<Candidate> = scored
            .iter()
            .map(|(mv, _, _, child_eval)| Candidate {
                san: mv.san.clone(),
                frequency: mv.frequency,
                // The child eval is from the *opponent's* perspective; negate to
                // score the move for the side that just moved.
                eval_cp: -score_to_cp(*child_eval),
            })
            .collect();

        for k in select_continuations(&candidates, config) {
            if nodes.len() >= config.max_nodes {
                break;
            }
            let (mv, child_fen, child_zobrist, child_eval) = &scored[k];
            let child_id = nodes.len();
            nodes.push(VariationNode {
                id: child_id,
                parent: Some(idx),
                san: Some(mv.san.clone()),
                fen: child_fen.clone(),
                zobrist: format!("{child_zobrist:016x}"),
                ply: ply + 1,
                eval: *child_eval,
                stats: Some(mv.clone()),
                eco: eco_for(*child_zobrist),
                children: Vec::new(),
            });
            nodes[idx].children.push(child_id);
            queue.push_back(child_id);
        }
    }

    Ok(VariationTree { nodes, root: 0 })
}

#[cfg(test)]
mod tests {
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
}
