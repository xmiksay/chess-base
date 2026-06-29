//! The reusable structured fact behind a move (issue #119, the "seam").
//!
//! [`MoveFact`] is the factual record the engine produced about one move: the
//! evaluation it conceded, the engine's preferred move and line, the material it
//! resolved to lose or win, and whether it missed or allowed a forced mate.
//! **Mode A** renders it as a terse, factual note via [`explain`]; **Mode B**
//! (the LLM annotation pass) is meant to consume the same struct as ground truth
//! so its strategic prose never drifts from the engine. Nothing here is an
//! opinion — every field is mechanically derived and verifiable.
//!
//! Pure and I/O-free: material is resolved by replaying the engine's PV through
//! [`crate::position`], the rest is arithmetic over two [`Score`]s.

use serde::Serialize;

use crate::engine::Score;
use crate::features::features_of_fen;
use crate::position::{apply_uci, CastlingMode};

use super::classify::{score_cp, Classification};

/// How many plies of the engine's continuation to replay when resolving the
/// material a move actually wins or loses (a hanging piece is captured a ply or
/// two later, not on the move itself).
const MATERIAL_PLIES: usize = 8;

/// Material swing (in pawns) at or beyond which a move is described as winning or
/// losing material. Below this the note stays about the evaluation alone.
const MATERIAL_THRESHOLD: i32 = 2;

/// The structured, verifiable facts about one played move. Built by
/// [`build_fact`] from the engine's evaluation of the position before and after
/// it; consumed by [`explain`] (Mode A) and the LLM annotation pass (Mode B).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MoveFact {
    /// 1-based ply index within the game.
    pub ply: usize,
    /// The move played, in normalized SAN (with check/mate suffix).
    pub san: String,
    /// Whether White played the move.
    pub mover_white: bool,
    /// Evaluation before the move, mover's perspective (mate mapped to ±1000).
    pub eval_before_cp: i32,
    /// Evaluation after the move, mover's perspective (mate mapped to ±1000).
    pub eval_after_cp: i32,
    /// The engine's preferred move in SAN, when it differs from the one played.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub best_san: Option<String>,
    /// The engine's principal variation from the position before, in SAN.
    pub best_line_san: Vec<String>,
    /// Material the move resolves to win (positive) or lose (negative) for the
    /// mover, in pawns, over the engine's continuation.
    pub material_swing: i32,
    /// A forced mate the mover had but did not play (its distance in moves).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub missed_mate_in: Option<u32>,
    /// A forced mate the move lets the opponent deliver (its distance in moves).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allowed_mate_in: Option<u32>,
}

/// Everything the pure fact builder needs about one move. The service fills it
/// from the engine results for the position before and after the move; the
/// builder derives material, mate flags and centipawn evals from it.
pub struct FactInput<'a> {
    pub ply: usize,
    pub san: &'a str,
    pub mover_white: bool,
    /// FEN before the move (position the mover chose from).
    pub fen_before: &'a str,
    /// FEN after the move (opponent to move).
    pub fen_after: &'a str,
    /// Best eval at `fen_before`, mover's perspective.
    pub best_before: Score,
    /// Eval after the played move, mover's perspective.
    pub after_played: Score,
    /// The engine's preferred move in SAN (when it differs from the played one).
    pub best_san: Option<String>,
    /// The engine's PV from `fen_before`, in SAN.
    pub best_line_san: Vec<String>,
    /// The engine's continuation from `fen_after`, in UCI (opponent's reply
    /// first) — replayed to resolve the material the move wins or loses.
    pub after_pv_uci: &'a [String],
    pub mode: CastlingMode,
}

/// Build the [`MoveFact`] for a move from the engine evaluation around it.
pub fn build_fact(input: FactInput) -> MoveFact {
    let missed_mate_in = match (input.best_before, input.after_played) {
        // The mover could force mate but the move played no longer does.
        (Score::Mate { value: b }, after) if b > 0 && !is_mate_for_mover(after) => {
            Some(b.unsigned_abs())
        }
        _ => None,
    };
    let allowed_mate_in = match input.after_played {
        // The move leaves the mover getting mated.
        Score::Mate { value } if value < 0 => Some(value.unsigned_abs()),
        _ => None,
    };

    let material_swing = resolve_material_swing(
        input.fen_before,
        input.fen_after,
        input.after_pv_uci,
        input.mover_white,
        input.mode,
    );

    MoveFact {
        ply: input.ply,
        san: input.san.to_string(),
        mover_white: input.mover_white,
        eval_before_cp: score_cp(input.best_before),
        eval_after_cp: score_cp(input.after_played),
        best_san: input.best_san,
        best_line_san: input.best_line_san,
        material_swing,
        missed_mate_in,
        allowed_mate_in,
    }
}

/// Whether a score (mover's perspective) is a forced mate *for* the mover.
fn is_mate_for_mover(score: Score) -> bool {
    matches!(score, Score::Mate { value } if value > 0)
}

/// White − Black material balance (pawns) for a position, or `None` if the FEN
/// can't be read. Reuses the pure feature layer's census (#30).
fn material_balance(fen: &str) -> Option<i32> {
    features_of_fen(fen).ok().map(|f| f.material_balance)
}

/// Net material change (pawns) from the mover's perspective, resolved by
/// replaying the engine's continuation a few plies past the move so a piece that
/// hangs is actually captured. Returns 0 when any position can't be read.
fn resolve_material_swing(
    fen_before: &str,
    fen_after: &str,
    after_pv_uci: &[String],
    mover_white: bool,
    mode: CastlingMode,
) -> i32 {
    let Some(before) = material_balance(fen_before) else {
        return 0;
    };
    let mut fen = fen_after.to_string();
    for uci in after_pv_uci.iter().take(MATERIAL_PLIES) {
        match apply_uci(&fen, uci, mode) {
            Ok((next, _)) => fen = next,
            Err(_) => break,
        }
    }
    let Some(after) = material_balance(&fen) else {
        return 0;
    };
    // `balance` is White − Black; flip for a Black mover so positive = good.
    let white_swing = after - before;
    if mover_white {
        white_swing
    } else {
        -white_swing
    }
}

/// Render a [`MoveFact`] (with its [`Classification`]) as a terse, factual
/// "why" note for the move list — no narrative, no engine numbers beyond the
/// evaluation delta. The whole string is mechanically derived, so Mode A never
/// editorialises.
pub fn explain(fact: &MoveFact, class: Classification) -> String {
    match class {
        Classification::Best => "Best move.".to_string(),
        Classification::Great => "Only move — every alternative is much worse.".to_string(),
        Classification::Good => "A solid move.".to_string(),
        Classification::Inaccuracy | Classification::Mistake | Classification::Blunder => {
            fault_note(fact)
        }
    }
}

/// Compose the note for an inaccuracy / mistake / blunder: the dominant reason
/// (missed/allowed mate, then material), then the evaluation swing and the move
/// the engine preferred.
fn fault_note(fact: &MoveFact) -> String {
    let mut parts: Vec<String> = Vec::new();

    if let Some(n) = fact.missed_mate_in {
        parts.push(format!("Missed mate in {n}."));
    } else if let Some(n) = fact.allowed_mate_in {
        parts.push(format!("Allows mate in {n}."));
    } else if let Some(phrase) = material_phrase(fact.material_swing) {
        parts.push(format!("{phrase}."));
    }

    let delta = signed_pawns(fact.eval_after_cp - fact.eval_before_cp);
    match &fact.best_san {
        Some(best) => parts.push(format!("{delta}: best was {best}.")),
        None => parts.push(format!("{delta}.")),
    }
    parts.join(" ")
}

/// Describe a material swing for the mover, or `None` when it's not material
/// enough to mention (the eval delta speaks for positional losses).
fn material_phrase(swing: i32) -> Option<String> {
    if swing <= -MATERIAL_THRESHOLD {
        Some(loss_phrase(-swing))
    } else if swing >= MATERIAL_THRESHOLD {
        Some(format!("wins {}", points_phrase(swing)))
    } else {
        None
    }
}

/// Phrase for material lost, coarse on purpose: the exact piece is ambiguous
/// from a net point count (a rook for a knight nets +2), so name the magnitude.
fn loss_phrase(points: i32) -> String {
    match points {
        ..=2 => "Drops a pawn".to_string(),
        3..=4 => "Loses a piece".to_string(),
        5..=8 => "Loses heavy material".to_string(),
        _ => "Hangs the queen".to_string(),
    }
}

fn points_phrase(points: i32) -> String {
    if points == 1 {
        "a pawn".to_string()
    } else {
        format!("{points} points of material")
    }
}

/// Format a centipawn delta as signed pawns: `-260` → `"-2.6"`, `+120` → `"+1.2"`.
fn signed_pawns(cp: i32) -> String {
    format!("{:+.1}", f64::from(cp) / 100.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::position::STARTPOS_FEN;

    const STD: CastlingMode = CastlingMode::Standard;

    fn cp(v: i32) -> Score {
        Score::Cp { value: v }
    }

    fn fact(class_input: FactInput) -> MoveFact {
        build_fact(class_input)
    }

    #[test]
    fn best_and_great_notes_are_positive() {
        let f = MoveFact {
            ply: 1,
            san: "Nf3".into(),
            mover_white: true,
            eval_before_cp: 30,
            eval_after_cp: 30,
            best_san: None,
            best_line_san: vec![],
            material_swing: 0,
            missed_mate_in: None,
            allowed_mate_in: None,
        };
        assert_eq!(explain(&f, Classification::Best), "Best move.");
        assert!(explain(&f, Classification::Great).starts_with("Only move"));
    }

    #[test]
    fn blunder_note_leads_with_delta_and_best_move() {
        let f = MoveFact {
            ply: 10,
            san: "Qd2".into(),
            mover_white: true,
            eval_before_cp: 40,
            eval_after_cp: -220,
            best_san: Some("Nf3".into()),
            best_line_san: vec!["Nf3".into()],
            material_swing: 0,
            missed_mate_in: None,
            allowed_mate_in: None,
        };
        let note = explain(&f, Classification::Blunder);
        assert!(note.contains("-2.6"), "delta in pawns: {note}");
        assert!(note.contains("best was Nf3"), "{note}");
    }

    #[test]
    fn missed_mate_takes_priority() {
        let f = MoveFact {
            ply: 5,
            san: "Qg4".into(),
            mover_white: true,
            eval_before_cp: 1000,
            eval_after_cp: 100,
            best_san: Some("Qh7#".into()),
            best_line_san: vec!["Qh7#".into()],
            material_swing: 0,
            missed_mate_in: Some(3),
            allowed_mate_in: None,
        };
        assert!(explain(&f, Classification::Blunder).starts_with("Missed mate in 3."));
    }

    #[test]
    fn material_loss_is_described() {
        let f = MoveFact {
            ply: 8,
            san: "Bd6".into(),
            mover_white: false,
            eval_before_cp: 10,
            eval_after_cp: -300,
            best_san: Some("Be7".into()),
            best_line_san: vec![],
            material_swing: -3,
            missed_mate_in: None,
            allowed_mate_in: None,
        };
        assert!(explain(&f, Classification::Mistake).starts_with("Loses a piece."));
    }

    #[test]
    fn build_fact_flags_missed_mate() {
        // Mover (White) had mate in 2 but the played move only keeps +1.0.
        let f = fact(FactInput {
            ply: 3,
            san: "Qg4",
            mover_white: true,
            fen_before: STARTPOS_FEN,
            fen_after: STARTPOS_FEN,
            best_before: Score::Mate { value: 2 },
            after_played: cp(100),
            best_san: Some("Qh7".into()),
            best_line_san: vec![],
            after_pv_uci: &[],
            mode: STD,
        });
        assert_eq!(f.missed_mate_in, Some(2));
        assert_eq!(f.eval_before_cp, 1000);
    }

    #[test]
    fn build_fact_flags_allowed_mate() {
        let f = fact(FactInput {
            ply: 4,
            san: "Kf2",
            mover_white: true,
            fen_before: STARTPOS_FEN,
            fen_after: STARTPOS_FEN,
            best_before: cp(20),
            after_played: Score::Mate { value: -1 },
            best_san: Some("Nf3".into()),
            best_line_san: vec![],
            after_pv_uci: &[],
            mode: STD,
        });
        assert_eq!(f.allowed_mate_in, Some(1));
    }

    #[test]
    fn material_swing_resolves_a_hanging_capture() {
        // Level material (knight each). White's knight sits on e3 en prise; in the
        // resolved line Black captures it (...Nxe3), so White is a knight down.
        let fen_before = "4k3/8/8/8/6n1/4N3/8/4K3 w - - 0 1";
        let fen_after = "4k3/8/8/8/6n1/4N3/8/4K3 b - - 1 1";
        let swing = resolve_material_swing(fen_before, fen_after, &["g4e3".into()], true, STD);
        assert_eq!(swing, -3, "White loses the knight captured on e3");
    }

    #[test]
    fn signed_pawns_formats_sign_and_decimal() {
        assert_eq!(signed_pawns(-260), "-2.6");
        assert_eq!(signed_pawns(120), "+1.2");
        assert_eq!(signed_pawns(0), "+0.0");
    }
}
