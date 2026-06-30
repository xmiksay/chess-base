//! Danger-map spine walk (issue #139, ADR-0026 increments 2–5): the driver that
//! turns a **repertoire** into a *tagged danger tree*. Where the best-line builder
//! ([`super::tree`]) grows the engine's top line, this walk is steered by a PGN
//! tree — the user's intended repertoire — and only ever asks the engine to
//! *adjudicate* the human moves the DB actually shows (ADR-0026: engine as
//! adjudicator, not author).
//!
//! For every **opponent-to-move** position reached on the spine it runs one
//! `analyse_multi` search and folds the result through the phase-1 classifier
//! ([`super::danger`]) into the three signals:
//!
//! - **reachability** — a frequent human reply that is *not* in the spine leaves
//!   the repertoire: an **Off-book** node (the move order we have no answer to);
//! - **trap** — the asymmetric refutation test on *our* move that created the
//!   position: bounded downside (opponent's best reply) vs baited upside
//!   (opponent's tempting second line) → **Weapon** or, when refuted, **Caution**;
//! - **only-move** — a wide MultiPV gap weighted by how often humans miss the
//!   unique reply in the DB → a **Weapon** narrow path.
//!
//! Layered for testability: the walk depends only on the [`MultiAnalyzer`] and
//! [`ContinuationSource`] seams, so it is unit-tested against fakes with no engine
//! or DB. The live engine/DB adapters live in the parent module ([`super`]).

use std::collections::{HashMap, VecDeque};

use serde::{Deserialize, Serialize};

use crate::engine::Analysis;
use crate::pgn_tree::MoveTree;
use crate::position::{apply_san, black_to_move, uci_to_san, CastlingMode};

use super::attack::{pawn_storm, AttackConfig, AttackSignal};
use super::danger::{is_only_move, only_move_gap, trap_verdict, DangerConfig, TrapVerdict};
use super::tree::{score_to_cp, ContinuationSource};

/// Multi-PV engine seam: up to *N* principal variations for a position, best
/// first (line 0 is the engine's best move). Mirrors
/// [`crate::engine::EngineService::analyse_multi`]; the movetime-per-variation
/// budget is baked into the live implementation, keeping the walk I/O- and
/// limit-free.
#[async_trait::async_trait]
pub trait MultiAnalyzer {
    async fn analyse_multi(&self, fen: &str) -> anyhow::Result<Vec<Analysis>>;
}

/// Which colour the repertoire is built for. At an *our* position the walk
/// follows the spine; at an *opponent* position it mines DB replies and runs the
/// engine. A repertoire from move 0 cannot reliably self-report its side, so the
/// caller states it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Side {
    White,
    Black,
}

/// How far and how wide to walk, plus the classifier thresholds. Engine search
/// limits live on the [`MultiAnalyzer`], not here.
#[derive(Debug, Clone, PartialEq)]
pub struct SpineConfig {
    /// Which side the repertoire plays.
    pub our_side: Side,
    /// Maximum plies from the root (root is ply 0).
    pub max_depth: usize,
    /// Drop human replies played in a smaller share of games than this (`0..=1`).
    pub min_frequency: f64,
    /// Cap on opponent replies expanded per position (most frequent first).
    pub max_replies: usize,
    /// Minimum share of humans that must *miss* the only move for an only-move
    /// position to count as a practical weapon (`0..=1`).
    pub min_miss_rate: f64,
    /// Phase-1 classifier thresholds (trap floors, only-move gap).
    pub danger: DangerConfig,
    /// Phase-5 pawn-storm thresholds (issue #142).
    pub attack: AttackConfig,
}

impl Default for SpineConfig {
    fn default() -> Self {
        Self {
            our_side: Side::White,
            max_depth: 8,
            min_frequency: 0.02,
            max_replies: 4,
            min_miss_rate: 0.3,
            danger: DangerConfig::default(),
            attack: AttackConfig::default(),
        }
    }
}

/// Why the spine move that created a position is dangerous (or why a position
/// left the repertoire).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DangerKind {
    /// Asymmetric refutation test fired (see [`DangerTag::trap`]).
    Trap,
    /// Wide MultiPV gap the opponent frequently misses.
    OnlyMove,
    /// A pawn storm toward our king in the opponent's best line (issue #142).
    Attack,
    /// A human reply with no answer in the spine (reachability break).
    OffBook,
}

/// What the user should *do* with a tagged node.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DangerRole {
    /// Recommend: passed the bounded-downside test, or a narrow path the
    /// opponent misses.
    Weapon,
    /// Warn: baits but the best reply refutes it (*do not play a blunder because
    /// there is a trap*).
    Caution,
    /// A move order the repertoire does not yet answer.
    OffBook,
}

/// The danger signal attached to one node, with the raw figures behind the
/// verdict so a later annotation pass can quote them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DangerTag {
    pub kind: DangerKind,
    pub role: DangerRole,
    /// Trap verdict on the move that reached this position, if computed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trap: Option<TrapVerdict>,
    /// `PV1 − PV2` gap (opponent's perspective) at the position, if a second line
    /// existed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub only_move_gap: Option<i32>,
    /// Share of DB games in which humans did *not* play the engine's best reply
    /// (`0..=1`), if the position was searched.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub miss_rate: Option<f64>,
    /// Pawn storm toward our king found in the opponent's best line (issue #142).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attack: Option<AttackSignal>,
}

/// One node of the tagged danger tree. Arena-allocated (`id` indexes
/// [`DangerTree::nodes`]) mirroring [`crate::pgn_tree::MoveTree`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DangerNode {
    pub id: usize,
    pub parent: Option<usize>,
    /// SAN of the move leading here; `None` only at the root.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub san: Option<String>,
    pub fen: String,
    /// Plies from the root.
    pub ply: usize,
    /// The danger signal on the move that reached this node, if any. Plain spine
    /// moves carry `None`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tag: Option<DangerTag>,
    pub children: Vec<usize>,
}

/// A walked, tagged repertoire tree — the output of the stage.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DangerTree {
    pub nodes: Vec<DangerNode>,
    pub root: usize,
}

/// Why walking the spine failed. Transport-agnostic.
#[derive(Debug, thiserror::Error)]
pub enum SpineError {
    #[error("invalid FEN: {0}")]
    InvalidFen(String),
    /// An analyzer or continuation-source failure, propagated verbatim.
    #[error(transparent)]
    Source(#[from] anyhow::Error),
}

/// One frame of the breadth-first walk: where we are in the spine, in the danger
/// tree, on the board, and how deep.
struct Frame {
    spine: usize,
    danger: usize,
    fen: String,
    ply: usize,
}

/// Walk `spine` (the intended repertoire) from `start_fen`, tagging every
/// opponent position the engine and DB flag as dangerous.
///
/// Breadth-first so the shallow, on-book lines are emitted first. At an *our*
/// position the walk descends every spine child (our prepared choices); at an
/// *opponent* position it searches once, tags the move that arrived there, then
/// follows the human replies — recursing into the ones the spine answers and
/// leaving the rest as **Off-book** leaves. Deterministic for deterministic seams.
pub async fn walk_danger_spine<A, S>(
    analyzer: &A,
    stats: &S,
    spine: &MoveTree,
    start_fen: &str,
    config: &SpineConfig,
    mode: CastlingMode,
) -> Result<DangerTree, SpineError>
where
    A: MultiAnalyzer + Sync,
    S: ContinuationSource + Sync,
{
    // Validate the root up front so a bad FEN fails fast and cleanly.
    black_to_move(start_fen, mode).map_err(|e| SpineError::InvalidFen(e.to_string()))?;

    let mut nodes = vec![DangerNode {
        id: 0,
        parent: None,
        san: None,
        fen: start_fen.to_string(),
        ply: 0,
        tag: None,
        children: Vec::new(),
    }];

    let mut queue = VecDeque::from([Frame {
        spine: spine.root,
        danger: 0,
        fen: start_fen.to_string(),
        ply: 0,
    }]);

    while let Some(frame) = queue.pop_front() {
        if frame.ply >= config.max_depth {
            continue;
        }

        let opponent_to_move = side_to_move(&frame.fen, mode)? != config.our_side;
        if !opponent_to_move {
            expand_our_moves(&mut nodes, &mut queue, spine, &frame, mode);
        } else {
            expand_opponent_moves(
                &mut nodes, &mut queue, analyzer, stats, spine, &frame, config, mode,
            )
            .await?;
        }
    }

    Ok(DangerTree { nodes, root: 0 })
}

/// At an *our* position: descend every spine child (our prepared moves). These
/// carry no tag of their own — the danger of a move is judged from the opponent
/// position it creates, set when that child is processed.
fn expand_our_moves(
    nodes: &mut Vec<DangerNode>,
    queue: &mut VecDeque<Frame>,
    spine: &MoveTree,
    frame: &Frame,
    mode: CastlingMode,
) {
    for &spine_child in &spine.nodes[frame.spine].children {
        let Some(san) = spine.nodes[spine_child].san.as_deref() else {
            continue;
        };
        let Ok((child_fen, _)) = apply_san(&frame.fen, san, mode) else {
            continue; // a repertoire move that no longer parses — skip
        };
        let id = push_node(
            nodes,
            frame.danger,
            san,
            child_fen.clone(),
            frame.ply + 1,
            None,
        );
        queue.push_back(Frame {
            spine: spine_child,
            danger: id,
            fen: child_fen,
            ply: frame.ply + 1,
        });
    }
}

/// At an *opponent* position: search once, tag the move that arrived here, then
/// follow the DB replies — on-book ones recurse down the spine, off-book ones
/// become tagged leaves.
#[allow(clippy::too_many_arguments)]
async fn expand_opponent_moves<A, S>(
    nodes: &mut Vec<DangerNode>,
    queue: &mut VecDeque<Frame>,
    analyzer: &A,
    stats: &S,
    spine: &MoveTree,
    frame: &Frame,
    config: &SpineConfig,
    mode: CastlingMode,
) -> Result<(), SpineError>
where
    A: MultiAnalyzer + Sync,
    S: ContinuationSource + Sync,
{
    let replies = stats.continuations(&frame.fen).await?;

    // Tag the move that created this position. The move-less root has no prior
    // move to judge, so it is never searched.
    if nodes[frame.danger].san.is_some() {
        let lines = analyzer.analyse_multi(&frame.fen).await?;
        nodes[frame.danger].tag = classify(&lines, &replies, &frame.fen, config, mode);
    }

    // Expected opponent moves we have a line against, by SAN.
    let answered: HashMap<&str, usize> = spine.nodes[frame.spine]
        .children
        .iter()
        .filter_map(|&c| spine.nodes[c].san.as_deref().map(|s| (s, c)))
        .collect();

    let mut kept: Vec<&_> = replies
        .iter()
        .filter(|r| r.frequency >= config.min_frequency)
        .collect();
    kept.sort_by(|a, b| {
        b.frequency
            .partial_cmp(&a.frequency)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.san.cmp(&b.san))
    });
    kept.truncate(config.max_replies);

    for reply in kept {
        let Ok((child_fen, _)) = apply_san(&frame.fen, &reply.san, mode) else {
            continue;
        };
        match answered.get(reply.san.as_str()) {
            Some(&spine_child) => {
                // On-book: a prepared reply — recurse back to our move.
                let id = push_node(
                    nodes,
                    frame.danger,
                    &reply.san,
                    child_fen.clone(),
                    frame.ply + 1,
                    None,
                );
                queue.push_back(Frame {
                    spine: spine_child,
                    danger: id,
                    fen: child_fen,
                    ply: frame.ply + 1,
                });
            }
            None => {
                // Off-book: a move order the repertoire does not answer.
                let tag = Some(DangerTag {
                    kind: DangerKind::OffBook,
                    role: DangerRole::OffBook,
                    trap: None,
                    only_move_gap: None,
                    miss_rate: None,
                    attack: None,
                });
                push_node(
                    nodes,
                    frame.danger,
                    &reply.san,
                    child_fen,
                    frame.ply + 1,
                    tag,
                );
            }
        }
    }
    Ok(())
}

/// Fold the multi-PV lines + DB replies for one opponent position into a tag for
/// the move that created it. Pure: all the engine-as-adjudicator logic lives here.
///
/// `lines` are from the **opponent's** perspective (best first). The trap test is
/// on *our* prior move, so its evals are negated to our perspective: bounded
/// downside is `−PV1` (opponent's best reply), baited upside `−PV2` (opponent's
/// tempting second line). Only-move reads the same gap, weighted by how often the
/// DB shows humans missing the engine's best reply. Attack (issue #142) scans the
/// opponent's best line for a pawn storm marching toward our king — a practical
/// danger this move concedes — and is the lowest-priority signal, surfaced as a
/// **Caution** only when no trap or narrow path already fired.
fn classify(
    lines: &[Analysis],
    replies: &[crate::search::report::MoveReport],
    fen: &str,
    config: &SpineConfig,
    mode: CastlingMode,
) -> Option<DangerTag> {
    let best = lines.first()?;
    let s1 = best.score;
    let s2 = lines.get(1).and_then(|l| l.score);

    let gap = only_move_gap(s1, s2);
    let miss = miss_rate(best, replies, fen, mode);

    // Trap: our move's worst case vs its baited case, both our perspective.
    let trap =
        s2.map(|second| trap_verdict(-score_to_cp(s1), -score_to_cp(Some(second)), &config.danger));

    let only = is_only_move(s1, s2, &config.danger) && miss.unwrap_or(0.0) >= config.min_miss_rate;

    // A bad PV simply yields no storm — the search already validated this FEN.
    let attack = pawn_storm(fen, &best.pv, mode, &config.attack)
        .ok()
        .flatten();

    let (kind, role) = match trap {
        Some(TrapVerdict::Weapon) => (DangerKind::Trap, DangerRole::Weapon),
        Some(TrapVerdict::HopeChess) => (DangerKind::Trap, DangerRole::Caution),
        _ if only => (DangerKind::OnlyMove, DangerRole::Weapon),
        _ if attack.is_some() => (DangerKind::Attack, DangerRole::Caution),
        _ => return None,
    };

    Some(DangerTag {
        kind,
        role,
        trap,
        only_move_gap: gap,
        miss_rate: miss,
        attack,
    })
}

/// Share of DB games that did *not* play the engine's best reply (`0..=1`).
/// `None` when the position was not searched or the best move cannot be mapped to
/// a SAN the DB reports.
fn miss_rate(
    best: &Analysis,
    replies: &[crate::search::report::MoveReport],
    fen: &str,
    mode: CastlingMode,
) -> Option<f64> {
    if best.bestmove.is_empty() {
        return None;
    }
    let best_san = uci_to_san(fen, &best.bestmove, mode).ok()?;
    let played = replies
        .iter()
        .find(|r| r.san == best_san)
        .map(|r| r.frequency)
        .unwrap_or(0.0);
    Some((1.0 - played).clamp(0.0, 1.0))
}

/// The colour to move in `fen`.
fn side_to_move(fen: &str, mode: CastlingMode) -> Result<Side, SpineError> {
    let black = black_to_move(fen, mode).map_err(|e| SpineError::InvalidFen(e.to_string()))?;
    Ok(if black { Side::Black } else { Side::White })
}

/// Push a child node onto the arena and link it under `parent`.
fn push_node(
    nodes: &mut Vec<DangerNode>,
    parent: usize,
    san: &str,
    fen: String,
    ply: usize,
    tag: Option<DangerTag>,
) -> usize {
    let id = nodes.len();
    nodes.push(DangerNode {
        id,
        parent: Some(parent),
        san: Some(san.to_string()),
        fen,
        ply,
        tag,
        children: Vec::new(),
    });
    nodes[parent].children.push(id);
    id
}

#[cfg(test)]
#[path = "spine_tests.rs"]
mod tests;
