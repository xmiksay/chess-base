//! Pure move-quality classification and game-accuracy aggregation (Mode A,
//! issue #119): given the engine's evaluation of a position before and after a
//! move, decide whether the move was best / great / good / an inaccuracy /
//! mistake / blunder, and roll the per-move losses up into per-side accuracy
//! and average centipawn loss.
//!
//! Like [`crate::features`] this is fully I/O-free and unit-tested: every
//! threshold is a plain function of two [`Score`]s, so the engine adapter and
//! the HTTP route stay thin callers.

use serde::Serialize;

use crate::engine::Score;

/// Logistic steepness mapping a centipawn evaluation to a win probability
/// (lichess' constant). A pawn (~100cp) is worth roughly +0.09 win probability
/// at level material; large advantages saturate, so trading +9 for +5 is not a
/// blunder while dropping +0.5 to −0.5 is.
const WIN_PROB_K: f64 = 0.00368208;

/// A score is clamped to ±[`MATE_CP`] centipawns when averaged, so a single
/// forced mate cannot dominate the centipawn-loss mean.
const MATE_CP: i32 = 1000;

/// Win-probability drop (mover's expected score lost) marking each bucket.
const BLUNDER_DROP: f64 = 0.30;
const MISTAKE_DROP: f64 = 0.20;
const INACCURACY_DROP: f64 = 0.10;
/// A move that concedes less than this counts as "best" even off the top line.
const BEST_DROP: f64 = 0.02;
/// For the played (best) move to be "great", every alternative must be at least
/// this much worse, and the position must be genuinely contested (not already
/// trivially won or lost, where any sensible move "holds").
const ONLY_MOVE_GAP: f64 = 0.15;
const CONTESTED_LOW: f64 = 0.10;
const CONTESTED_HIGH: f64 = 0.90;

/// A move's quality, derived purely from the engine evaluation before and after
/// it. Each variant is a discrete tag the UI renders and the explanation builder
/// phrases; serialised `snake_case` for the JSON API.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Classification {
    /// The engine's top choice, or within a hair of it.
    Best,
    /// The only move that holds the position — every alternative collapses.
    Great,
    /// A sound move that keeps the evaluation.
    Good,
    /// `?!` — a small but real concession.
    Inaccuracy,
    /// `?` — a clear error.
    Mistake,
    /// `??` — a losing error.
    Blunder,
}

impl Classification {
    /// The conventional NAG glyph for this judgement, or `None` for moves that
    /// carry no annotation symbol (best/good play).
    pub fn nag(self) -> Option<u8> {
        match self {
            Classification::Great => Some(1),      // `!`
            Classification::Inaccuracy => Some(6), // `?!`
            Classification::Mistake => Some(2),    // `?`
            Classification::Blunder => Some(4),    // `??`
            Classification::Best | Classification::Good => None,
        }
    }
}

/// Win probability in `[0, 1]` for the side to move, from a [`Score`] given in
/// **that side's** perspective. A forced mate saturates to a (near-)certain
/// result regardless of distance.
pub fn win_prob(score: Score) -> f64 {
    match score {
        Score::Mate { value } => {
            if value >= 0 {
                1.0
            } else {
                0.0
            }
        }
        Score::Cp { value } => 1.0 / (1.0 + (-WIN_PROB_K * f64::from(value)).exp()),
    }
}

/// Map a [`Score`] to a centipawn number for averaging, clamping a mate to
/// ±[`MATE_CP`] so it cannot dominate the mean.
pub fn score_cp(score: Score) -> i32 {
    match score {
        Score::Cp { value } => value.clamp(-MATE_CP, MATE_CP),
        Score::Mate { value } => {
            if value >= 0 {
                MATE_CP
            } else {
                -MATE_CP
            }
        }
    }
}

/// Classify a played move from the engine's evaluation, all in the **mover's**
/// perspective: `best_before` is the best achievable eval at the position before
/// the move, `after_played` the eval after the move actually played,
/// `played_is_best` whether it matched the engine's top move, and `second_best`
/// the runner-up's eval (when a MultiPV search found one).
pub fn classify(
    best_before: Score,
    after_played: Score,
    played_is_best: bool,
    second_best: Option<Score>,
) -> Classification {
    if played_is_best {
        // The only move that holds: alternatives collapse, in a contested spot.
        if let Some(second) = second_best {
            let wp = win_prob(best_before);
            let gap = wp - win_prob(second);
            if gap >= ONLY_MOVE_GAP && (CONTESTED_LOW..=CONTESTED_HIGH).contains(&wp) {
                return Classification::Great;
            }
        }
        return Classification::Best;
    }

    let drop = (win_prob(best_before) - win_prob(after_played)).max(0.0);
    if drop >= BLUNDER_DROP {
        Classification::Blunder
    } else if drop >= MISTAKE_DROP {
        Classification::Mistake
    } else if drop >= INACCURACY_DROP {
        Classification::Inaccuracy
    } else if drop <= BEST_DROP {
        Classification::Best
    } else {
        Classification::Good
    }
}

/// One move's cost to its side: the centipawn lost versus the best move and the
/// win-probability lost (0..=100), tagged with the [`Classification`]. The pure
/// input to [`summarize`].
#[derive(Debug, Clone, Copy)]
pub struct MoveCost {
    /// Whether White played this move.
    pub white: bool,
    pub cp_loss: u32,
    /// Win probability lost, in percentage points (0..=100).
    pub win_pct_loss: f64,
    pub classification: Classification,
}

impl MoveCost {
    /// Derive a move's cost from the engine evaluation around it (mover's
    /// perspective), so the service builds costs without duplicating the math.
    pub fn from_eval(
        white: bool,
        best_before: Score,
        after_played: Score,
        classification: Classification,
    ) -> Self {
        let cp_loss = (score_cp(best_before) - score_cp(after_played)).max(0) as u32;
        let win_pct_loss = ((win_prob(best_before) - win_prob(after_played)) * 100.0).max(0.0);
        Self {
            white,
            cp_loss,
            win_pct_loss,
            classification,
        }
    }
}

/// One side's review summary: average centipawn loss, accuracy percentage, and
/// the count of each error grade.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SideSummary {
    pub acpl: u32,
    /// Accuracy percentage (0..=100), higher is better.
    pub accuracy: f64,
    pub inaccuracies: u32,
    pub mistakes: u32,
    pub blunders: u32,
}

/// Per-side roll-up of a reviewed game.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ReviewSummary {
    pub white: SideSummary,
    pub black: SideSummary,
}

/// Per-move accuracy from the win-percent lost (lichess' formula), clamped to
/// `[0, 100]`. A perfect move (no loss) scores 100; the curve falls off fast.
fn accuracy_from_winloss(win_pct_loss: f64) -> f64 {
    (103.1668 * (-0.04354 * win_pct_loss).exp() - 3.1669).clamp(0.0, 100.0)
}

/// Round to one decimal place for a stable JSON payload.
fn round1(x: f64) -> f64 {
    (x * 10.0).round() / 10.0
}

/// Aggregate per-move costs into per-side [`SideSummary`] roll-ups.
pub fn summarize(costs: &[MoveCost]) -> ReviewSummary {
    ReviewSummary {
        white: side_summary(costs, true),
        black: side_summary(costs, false),
    }
}

fn side_summary(costs: &[MoveCost], white: bool) -> SideSummary {
    let side: Vec<&MoveCost> = costs.iter().filter(|c| c.white == white).collect();
    let n = side.len();
    if n == 0 {
        // No moves played by this side ⇒ nothing to fault: a flawless 100%.
        return SideSummary {
            acpl: 0,
            accuracy: 100.0,
            inaccuracies: 0,
            mistakes: 0,
            blunders: 0,
        };
    }
    let acpl = side.iter().map(|c| c.cp_loss).sum::<u32>() / n as u32;
    let accuracy = side
        .iter()
        .map(|c| accuracy_from_winloss(c.win_pct_loss))
        .sum::<f64>()
        / n as f64;
    let count = |g: Classification| side.iter().filter(|c| c.classification == g).count() as u32;
    SideSummary {
        acpl,
        accuracy: round1(accuracy),
        inaccuracies: count(Classification::Inaccuracy),
        mistakes: count(Classification::Mistake),
        blunders: count(Classification::Blunder),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cp(v: i32) -> Score {
        Score::Cp { value: v }
    }
    fn mate(v: i32) -> Score {
        Score::Mate { value: v }
    }

    #[test]
    fn win_prob_is_centered_and_monotonic() {
        assert!((win_prob(cp(0)) - 0.5).abs() < 1e-9);
        assert!(win_prob(cp(300)) > win_prob(cp(100)));
        assert_eq!(win_prob(mate(3)), 1.0);
        assert_eq!(win_prob(mate(-2)), 0.0);
    }

    #[test]
    fn playing_the_best_move_is_best() {
        assert_eq!(
            classify(cp(30), cp(30), true, Some(cp(20))),
            Classification::Best
        );
    }

    #[test]
    fn sole_holding_move_is_great() {
        // Best keeps the game level; every alternative loses badly ⇒ only move.
        let c = classify(cp(20), cp(20), true, Some(cp(-400)));
        assert_eq!(c, Classification::Great);
    }

    #[test]
    fn only_move_label_needs_a_contested_position() {
        // Already winning by a mile: the "only move" gap is meaningless.
        let c = classify(cp(1200), cp(1200), true, Some(cp(200)));
        assert_eq!(c, Classification::Best);
    }

    #[test]
    fn small_concession_is_an_inaccuracy() {
        // ~+0.5 → −0.0: about a 9–10% win-prob drop.
        let c = classify(cp(50), cp(-60), false, None);
        assert_eq!(c, Classification::Inaccuracy);
    }

    #[test]
    fn handing_over_a_won_game_is_a_blunder() {
        // Winning (+4) to losing (−4): a >30% win-prob swing.
        let c = classify(cp(400), cp(-400), false, None);
        assert_eq!(c, Classification::Blunder);
    }

    #[test]
    fn trading_down_a_huge_edge_is_not_punished() {
        // +15 to +11: both completely winning, so the win-prob curve has
        // saturated and a 400cp give-back is negligible ⇒ still best/good.
        let c = classify(cp(1500), cp(1100), false, None);
        assert!(matches!(c, Classification::Best | Classification::Good));
    }

    #[test]
    fn missing_a_mate_is_a_blunder() {
        let c = classify(mate(2), cp(50), false, None);
        assert_eq!(c, Classification::Blunder);
    }

    #[test]
    fn summary_rolls_up_per_side() {
        let costs = vec![
            MoveCost::from_eval(true, cp(30), cp(30), Classification::Best),
            MoveCost::from_eval(false, cp(400), cp(-400), Classification::Blunder),
            MoveCost::from_eval(true, cp(50), cp(-60), Classification::Inaccuracy),
        ];
        let s = summarize(&costs);
        assert_eq!(s.black.blunders, 1);
        assert_eq!(s.white.inaccuracies, 1);
        // White conceded something, Black threw a won game: White is more accurate.
        assert!(s.white.accuracy > s.black.accuracy);
        // Black's lone blunder caps centipawn loss at the mate ceiling band.
        assert!(s.black.acpl > s.white.acpl);
    }

    #[test]
    fn flawless_side_scores_full_accuracy() {
        let costs = vec![MoveCost::from_eval(
            true,
            cp(20),
            cp(20),
            Classification::Best,
        )];
        let s = summarize(&costs);
        assert_eq!(s.black.accuracy, 100.0);
        assert_eq!(s.black.acpl, 0);
        assert!(s.white.accuracy > 99.0);
    }

    #[test]
    fn classification_nag_glyphs() {
        assert_eq!(Classification::Blunder.nag(), Some(4));
        assert_eq!(Classification::Great.nag(), Some(1));
        assert_eq!(Classification::Best.nag(), None);
    }
}
