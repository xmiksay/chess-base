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
use std::collections::{HashMap, HashSet, VecDeque};

use anyhow::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::engine::Score;
use crate::openings::opening_of_zobrist;
use crate::position::{apply_san, zobrist_of_fen, CastlingMode};
use crate::search::report::{EcoInfo, MoveReport};
use crate::study_gen::features::{concepts_of_fen_with, Concepts};

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
    /// Maximum continuations kept at any node. Caps tree breadth. Overridden
    /// per-depth by [`max_children_by_depth`](Self::max_children_by_depth) when
    /// that is set.
    pub max_children: usize,
    /// Per-depth override for [`max_children`](Self::max_children): the child cap
    /// for a node at ply *i* is `max_children_by_depth[i]`, with the **last entry
    /// repeating** for any deeper ply. Lets branching **taper with depth** — broad
    /// near the root (cover the opponent's main tries), narrowing to the principal
    /// line deep (issue #160), e.g. `[3,3,2,2,1,1,1,1,1,1]`. `None` ⇒ every depth
    /// uses the scalar `max_children` (uniform branching; backward compatible).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_children_by_depth: Option<Vec<usize>>,
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
            max_children_by_depth: None,
            max_nodes: 64,
            min_frequency: 0.05,
            eval_margin_cp: 100,
        }
    }
}

impl TreeConfig {
    /// Effective per-node child cap for a node at `ply` (the depth of the node
    /// whose children are being selected; root = 0). With
    /// [`max_children_by_depth`](Self::max_children_by_depth) set and non-empty,
    /// indexes that vector — the last entry repeats for any deeper ply — so
    /// branching tapers with depth. Otherwise every depth uses the scalar
    /// [`max_children`](Self::max_children).
    pub fn max_children_at(&self, ply: usize) -> usize {
        match &self.max_children_by_depth {
            Some(caps) if !caps.is_empty() => caps[ply.min(caps.len() - 1)],
            _ => self.max_children,
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
    /// Strategic concepts (pawn structure, key squares, files, king safety,
    /// material imbalance) for this position — the issue #30 feature layer.
    #[serde(default, skip_serializing_if = "Concepts::is_empty")]
    pub concepts: Concepts,
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
    select_continuations_capped(candidates, config, config.max_children)
}

/// As [`select_continuations`] but with an explicit breadth cap, so the builder
/// can taper width per depth ([`TreeConfig::max_children_at`]) while reusing the
/// same frequency + eval-margin pruning. The frequency floor and eval margin
/// come from `config`; only the final breadth truncation uses `max_children`.
fn select_continuations_capped(
    candidates: &[Candidate],
    config: &TreeConfig,
    max_children: usize,
) -> Vec<usize> {
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
    kept.truncate(max_children);
    kept
}

/// ECO classification for a Zobrist key, shaped as the DB report's [`EcoInfo`].
/// Shared with the danger-study converter ([`super::danger_generate`]), which
/// rebuilds variation nodes from a tagged danger tree.
pub(super) fn eco_for(zobrist: u64) -> Option<EcoInfo> {
    opening_of_zobrist(zobrist).map(|o| EcoInfo {
        eco: o.eco.to_string(),
        name: o.name.to_string(),
    })
}

/// Build a bounded, pruned, tagged variation tree from `start_fen`.
///
/// Breadth-first: each visited position's DB continuations are filtered by
/// frequency, the survivors evaluated, then [`select_continuations`] picks the
/// kept moves. Expansion stops at `max_depth`, the per-node breadth cap, and the
/// global `max_nodes` budget. The breadth cap is [`TreeConfig::max_children_at`]
/// at the node's ply, so `max_children_by_depth` can taper width with depth
/// (broad near the root, narrow on deep main lines, #160); unset it is the
/// uniform `max_children`. Deterministic for deterministic seams.
///
/// `castling` is the variant's castling mode (e.g. [`CastlingMode::Chess960`]
/// for a Fischer-Random start): it drives the root Zobrist and every `apply_san`
/// so node hashes, ECO lookup and transposition matching line up with the
/// variant's real positions.
///
/// Transposition dedup within a single build: each unique position (keyed by its
/// Zobrist) is evaluated by the engine at most once and its subtree expanded at
/// most once. A move transposing into an already-enqueued position still gets a
/// node (so it stays visible) but becomes a leaf rather than a duplicate subtree.
pub async fn build_tree<E, S>(
    eval: &E,
    stats: &S,
    start_fen: &str,
    config: &TreeConfig,
    castling: CastlingMode,
) -> Result<VariationTree, TreeError>
where
    E: Evaluator + Sync,
    S: ContinuationSource + Sync,
{
    let root_zobrist =
        zobrist_of_fen(start_fen, castling).map_err(|e| TreeError::InvalidFen(e.to_string()))?;

    // Each unique position is evaluated at most once per build.
    let mut eval_cache: HashMap<u64, Option<Score>> = HashMap::new();
    let root_eval = eval.eval(start_fen).await?;
    eval_cache.insert(root_zobrist, root_eval);

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
        concepts: concepts_of_fen_with(start_fen, castling).unwrap_or_default(),
        children: Vec::new(),
    }];

    // Positions already enqueued for expansion; a transposition to one of these
    // becomes a leaf instead of a duplicate subtree.
    let mut enqueued: HashSet<u64> = HashSet::new();
    enqueued.insert(root_zobrist);

    let mut queue = VecDeque::from([0usize]);
    while let Some(idx) = queue.pop_front() {
        let ply = nodes[idx].ply;
        if ply >= config.max_depth || nodes.len() >= config.max_nodes {
            continue;
        }
        let fen = nodes[idx].fen.clone();

        // Pre-filter on frequency before spending an engine eval per move, then
        // evaluate each survivor's resulting position (cached per unique position).
        let mut scored: Vec<(MoveReport, String, u64, Option<Score>)> = Vec::new();
        for mv in stats.continuations(&fen).await? {
            if mv.frequency < config.min_frequency {
                continue;
            }
            let (child_fen, child_zobrist) = match apply_san(&fen, &mv.san, castling) {
                Ok(pair) => pair,
                Err(_) => continue, // a move the DB has that no longer parses — skip
            };
            let child_eval = match eval_cache.get(&child_zobrist) {
                Some(cached) => *cached,
                None => {
                    let e = eval.eval(&child_fen).await?;
                    eval_cache.insert(child_zobrist, e);
                    e
                }
            };
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

        // Tapering: this node's ply sets how many children survive, so the tree
        // can fan out near the root and narrow on deep main lines (issue #160).
        let cap = config.max_children_at(ply);
        for k in select_continuations_capped(&candidates, config, cap) {
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
                concepts: concepts_of_fen_with(child_fen, castling).unwrap_or_default(),
                children: Vec::new(),
            });
            nodes[idx].children.push(child_id);
            // Only expand the first occurrence of a position; a transposition to
            // an already-enqueued position stays a visible leaf.
            if enqueued.insert(*child_zobrist) {
                queue.push_back(child_id);
            }
        }
    }

    Ok(VariationTree { nodes, root: 0 })
}

#[cfg(test)]
#[path = "tree_tests.rs"]
mod tests;
