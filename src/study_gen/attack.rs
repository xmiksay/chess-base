//! Pure attack-plan detection (issue #142, ADR-0026 signal 4) — the danger map's
//! deferred fourth signal: a recurring threat-generating **pawn storm**.
//!
//! Built on the plan tracer ([`crate::plans`], ADR-0017): trace the side-to-
//! move's pawns across an engine principal variation and report when one of them
//! marches toward the enemy king — the classic flank storm (`h4-h5`, `g4-g5`, …)
//! whose threat the static one-ply [`crate::threats`] scan never sees. Unlike a
//! trap or a narrow path, an attacking storm is *practically* dangerous yet often
//! objectively double-edged, so the danger map attaches it as a **Caution** note
//! rather than a recommended Weapon (ADR-0026: "a dangerous-but-unsound attacking
//! plan"). The complementary heuristic for the opponent's *tempting* reply is the
//! still-open question — surfaced via the study-assistant chat in v1, not here.
//!
//! Pure and I/O-free; the spine walk ([`super::spine`]) is the only caller.

use serde::{Deserialize, Serialize};
use shakmaty::Position;

use crate::plans::{plan_from_pv, DEFAULT_MAX_MOVES};
use crate::position::{king_square, position_from_fen, CastlingMode, PositionError};

/// Tunables for the pawn-storm detector, deliberately easy to retune.
/// `serde(default)` so a generate request can carry partial overrides over the
/// defaults (issue #141).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AttackConfig {
    /// Minimum forward pushes by one pawn along the line to count as a storm.
    /// Two (e.g. `h4` then `h5`) is the classic flank advance.
    pub min_advances: usize,
    /// The pawn must finish within this many files of the enemy king to count as
    /// aimed at it — an `h`-pawn against a king on `g8` is one file away.
    pub king_zone_files: u32,
}

impl Default for AttackConfig {
    fn default() -> Self {
        Self {
            min_advances: 2,
            king_zone_files: 2,
        }
    }
}

/// A detected pawn storm: the storming pawn, its traced path, and how far it
/// advanced. Carried on the danger tag so a later annotation pass can quote it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttackSignal {
    /// Colour-cased FEN char of the storming pawn (`'P'` / `'p'`).
    pub pawn: char,
    /// The pawn's squares across the line, origin first (`["h2","h4","h5"]`).
    pub path: Vec<String>,
    /// Forward pushes along the path (`path.len() - 1`).
    pub advances: usize,
}

/// Detect a pawn storm by the side to move in `start_fen` toward the enemy king,
/// across the engine line `pv_uci`.
///
/// Reuses [`plan_from_pv`] to trace the side-to-move's pieces, then keeps the
/// pawn that pushed at least `min_advances` times and finished within
/// `king_zone_files` of the enemy king. Returns the furthest-advanced such pawn —
/// the spearhead — or `None` when no pawn qualifies. Errors only on an invalid
/// `start_fen`; a truncated or illegal `pv_uci` simply traces what it can.
pub fn pawn_storm(
    start_fen: &str,
    pv_uci: &[String],
    mode: CastlingMode,
    config: &AttackConfig,
) -> Result<Option<AttackSignal>, PositionError> {
    let pos = position_from_fen(start_fen, mode)?;
    // No enemy king ⇒ nothing to storm (never happens in a legal position).
    let Some(king_sq) = king_square(pos.board(), pos.turn().other()) else {
        return Ok(None);
    };
    let king_file = file_index(&king_sq.to_string());

    let plan = plan_from_pv(start_fen, pv_uci, DEFAULT_MAX_MOVES, mode)?;

    let mut best: Option<AttackSignal> = None;
    for traj in plan.trajectories {
        if !is_pawn(traj.piece) {
            continue;
        }
        let advances = traj.squares.len().saturating_sub(1);
        if advances < config.min_advances {
            continue;
        }
        let Some(last) = traj.squares.last() else {
            continue;
        };
        if king_file.abs_diff(file_index(last)) > config.king_zone_files {
            continue;
        }
        if best.as_ref().is_none_or(|b| advances > b.advances) {
            best = Some(AttackSignal {
                pawn: traj.piece,
                path: traj.squares,
                advances,
            });
        }
    }
    Ok(best)
}

fn is_pawn(piece: char) -> bool {
    piece == 'P' || piece == 'p'
}

/// 0-based file index of an algebraic square (`"h5"` → 7). Squares originate from
/// `shakmaty`, so the leading file byte is always `b'a'..=b'h'`.
fn file_index(square: &str) -> u32 {
    let file = square.as_bytes().first().copied().unwrap_or(b'a');
    u32::from(file.saturating_sub(b'a'))
}

#[cfg(test)]
#[path = "attack_tests.rs"]
mod tests;
