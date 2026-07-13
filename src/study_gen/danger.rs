//! Danger-map classifier (ADR-0026, issue #131): the pure scoring core of the
//! opening-builder's "danger map" mode. Where the best-line builder (`tree.rs`)
//! keeps engine-best moves, the danger map keeps the moves that are
//! *practically* hard for the opponent — traps with a bounded downside and
//! narrow only-move paths — because those, not the engine's top line, are what
//! makes an opening study worth more than scrolling the analysis board.
//!
//! The engine is the **adjudicator, not the author** (ADR-0009): a move that
//! merely *baits* but blunders when refuted is rejected here as hope-chess,
//! encoding the rule *do not play a blunder because there is a trap*.
//!
//! Pure and I/O-free. Callers feed centipawn evals already normalised to **our**
//! (the side choosing the move) perspective — larger is better for us — and this
//! module decides the verdict. The reachability and attack signals from ADR-0026,
//! and the PGN-spine walk that produces these evals, are out of this slice and
//! handled by the orchestrator (issue #131, increments 2–5).

use serde::{Deserialize, Serialize};

use crate::engine::Score;

use super::tree::score_to_cp;

/// Tunable thresholds for the classifier, all in centipawns from our
/// perspective. Defaults are the ADR-0026 starting points — deliberately easy to
/// retune once measured on real games. `serde(default)` so a generate request can
/// carry partial overrides over the defaults (issue #141).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct DangerConfig {
    /// After the opponent's **best** refutation we must stay at or above this to
    /// call a trap a *weapon*. Slightly worse is fine; a blunder is not.
    pub downside_floor_cp: i32,
    /// After the **tempting** reply we must reach at least this for the move to
    /// carry real trap upside.
    pub baited_upside_cp: i32,
    /// `PV1 − PV2` gap (our perspective) at or above which the best move is the
    /// opponent's only adequate reply — a narrow path.
    pub only_move_gap_cp: i32,
    /// A `Weapon` candidate is confirmed one ply deeper: our eval, from our
    /// perspective, after the opponent's best reply is actually played. Below
    /// this floor the trap is refuted and downgraded to `HopeChess` (issue
    /// #175) — bounded on the shallow root eval alone, a follow-up this bad
    /// means the opponent's best reply leaves *us* worse than "slightly worse",
    /// not just the opponent. Deliberately stricter than `downside_floor_cp`
    /// and aligned with `review.rs`'s mistake-magnitude buckets.
    pub follow_up_floor_cp: i32,
}

impl Default for DangerConfig {
    fn default() -> Self {
        Self {
            downside_floor_cp: -80,
            baited_upside_cp: 150,
            only_move_gap_cp: 120,
            follow_up_floor_cp: -200,
        }
    }
}

/// Outcome of the asymmetric trap test for one candidate move.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TrapVerdict {
    /// Bounded downside (`>= downside_floor_cp`) **and** a baiting upside
    /// (`>= baited_upside_cp`): a real, recommendable trap.
    Weapon,
    /// Baits, but the best refutation drops us below the downside floor —
    /// hope-chess. Rejected: *do not play a blunder because there is a trap.*
    HopeChess,
    /// No baiting upside worth a trap; nothing special about this move.
    Quiet,
}

/// Classify a candidate move from the asymmetric refutation test.
///
/// Both inputs are centipawn evals from **our** perspective:
/// - `if_refuted_cp` — eval after the opponent's *best* reply (our worst case),
/// - `if_baited_cp` — eval after the opponent's *tempting* reply.
///
/// A move is a [`TrapVerdict::Weapon`] only when the downside is bounded **and**
/// the baited upside is large. A baiting move whose refutation leaves us below
/// the floor is [`TrapVerdict::HopeChess`] — never recommended.
pub fn trap_verdict(if_refuted_cp: i32, if_baited_cp: i32, config: &DangerConfig) -> TrapVerdict {
    if if_baited_cp < config.baited_upside_cp {
        return TrapVerdict::Quiet;
    }
    if if_refuted_cp >= config.downside_floor_cp {
        TrapVerdict::Weapon
    } else {
        TrapVerdict::HopeChess
    }
}

/// Confirm a `Weapon` candidate against our own follow-up, one ply deeper than
/// [`trap_verdict`] looks (issue #175). `trap_verdict` only bounds the downside
/// implied by the opponent's best-reply eval; it never checks that *we* still
/// have a decent position once that reply is actually played — a shallow root
/// search can pass the floor test while the deeper, played-out position is
/// already lost. `follow_up_cp` is our eval, our perspective, at the position
/// reached after the opponent's best reply; `None` when it could not be
/// computed (no PV to follow, or the follow-up search failed) leaves the
/// verdict unchanged — nothing on hand to reveal a refutation. Non-`Weapon`
/// verdicts pass through untouched: only a recommended trap needs confirming.
pub fn confirm_weapon(
    verdict: TrapVerdict,
    follow_up_cp: Option<i32>,
    config: &DangerConfig,
) -> TrapVerdict {
    if verdict != TrapVerdict::Weapon {
        return verdict;
    }
    match follow_up_cp {
        Some(cp) if cp < config.follow_up_floor_cp => TrapVerdict::HopeChess,
        _ => verdict,
    }
}

/// The centipawn gap between the best and second-best reply, from the side-to-
/// move's perspective (both scores are of the *same* position, so they share a
/// perspective). `None` when there is no second line — the literal one-legal-move
/// case is left to the orchestrator, which counts legal moves directly (the
/// existing `only_move` claim in `annotate.rs`).
pub fn only_move_gap(best: Option<Score>, second: Option<Score>) -> Option<i32> {
    second.map(|s| score_to_cp(best).saturating_sub(score_to_cp(Some(s))))
}

/// Whether the opponent's best reply is their *only* adequate one: a second line
/// exists and is at least `only_move_gap_cp` worse than the best. A wider gap is
/// a narrower path and a more dangerous position to face practically.
pub fn is_only_move(best: Option<Score>, second: Option<Score>, config: &DangerConfig) -> bool {
    only_move_gap(best, second).is_some_and(|gap| gap >= config.only_move_gap_cp)
}

#[cfg(test)]
#[path = "danger_tests.rs"]
mod tests;
