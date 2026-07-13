//! The tagged danger tree: the output shape of the [`super::spine`] walk
//! (ADR-0026). [`DangerNode`]/[`DangerTree`] are an arena mirroring
//! [`crate::pgn_tree::MoveTree`]'s shape; [`DangerTag`] is the danger signal one
//! node carries — kind, role, and the raw figures behind the verdict (trap
//! verdict, only-move gap, miss rate, pawn-storm attack, and the position's own
//! White-perspective eval, issue #177) — so a later pass (LLM prose, or a study
//! merge that grafts the tree) can quote them instead of re-deriving them.
//!
//! Split out of `spine.rs` (the walk driver) to keep that file under the
//! project's line cap; the walk still owns building and tagging this tree.

use serde::{Deserialize, Serialize};

use crate::pgn_tree::Eval;

use super::attack::AttackSignal;
use super::danger::TrapVerdict;

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
    /// White-perspective engine eval of the position this node reaches — the PGN
    /// `[%eval]` convention (issue #177) — so a Weapon at +0.3 and one at −0.5
    /// aren't indistinguishable in the overlay or the merge-danger graft. `None`
    /// for an Off-book node: no search ran on its own position, only on the
    /// parent move that flagged it missing.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub eval: Option<Eval>,
}

/// One node of the tagged danger tree. Arena-allocated (`id` indexes
/// [`DangerTree::nodes`]) mirroring [`crate::pgn_tree::MoveTree`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DangerNode {
    pub id: usize,
    pub parent: Option<usize>,
    /// SAN of the move leading here; `None` only at the root. `default` so a
    /// serialized tree (root omits `san`) round-trips back through deserialize —
    /// the danger overlay POSTs it to `/api/studies/{id}/merge-danger` (ADR-0032).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub san: Option<String>,
    pub fen: String,
    /// Plies from the root.
    pub ply: usize,
    /// The danger signal on the move that reached this node, if any. Plain spine
    /// moves carry `None`. `default` for the same round-trip reason as `san`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tag: Option<DangerTag>,
    pub children: Vec<usize>,
}

/// A walked, tagged repertoire tree — the output of the stage.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DangerTree {
    pub nodes: Vec<DangerNode>,
    pub root: usize,
}
