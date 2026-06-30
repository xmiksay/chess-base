//! Full-game engine review (Mode A, issue #119): replay a stored game and run
//! the engine once per position at a fixed depth, then turn each ply into a
//! classified, explained [`MoveReview`] plus a per-side [`ReviewSummary`].
//!
//! The engine I/O ([`review_game`]) is a thin shell: it gathers one
//! [`PosEval`] per position from the shared [`EngineService`] pool, then hands
//! the whole sequence to the pure [`assemble`] step, which does all the
//! classification, fact-building and explanation. `assemble` is unit-tested with
//! synthetic evals — no engine process required.
//!
//! A review is a long run of sequential searches. It shares the one-shot engine
//! pool with the batch study generator and the MCP `engine_analyse` tool, but
//! **not** the interactive analysis WebSocket, which keeps its own per-socket
//! engine (`server/engine_ws.rs`) — so a full-game pass never starves live
//! analysis (ADR 0014).

use serde::Serialize;

use crate::engine::{Analysis, EngineService, Limits, Score};
use crate::features::features_of_fen;
use crate::position::{self, black_to_move, san_to_uci, uci_to_san, CastlingMode, Ply};

use super::classify::{classify, score_cp, summarize, Classification, MoveCost, ReviewSummary};
use super::explain::{build_fact, explain, FactInput};

/// How many MultiPV lines each position is searched with: the best move plus its
/// closest rival, enough to spot an "only move" and rank the played move.
const REVIEW_MULTIPV: u16 = 2;

/// Plies of the engine's PV kept (in SAN) for display alongside the best move.
const PV_SAN_PLIES: usize = 6;

/// Why a game review failed. Transport-agnostic; the route maps each onto a
/// status and a client-safe message.
#[derive(Debug, thiserror::Error)]
pub enum ReviewError {
    /// The stored game could not be parsed or replayed (bad PGN / illegal move).
    #[error("{0}")]
    BadGame(String),
    /// The engine pool failed mid-review. Internal — never surfaced raw.
    #[error(transparent)]
    Engine(#[from] anyhow::Error),
}

/// One position's engine result, distilled to the few lines the review needs.
/// Empty `lines` marks a terminal position (mate/stalemate) that was not
/// searched. Built by [`review_game`] from [`EngineService::analyse_multi`].
#[derive(Debug, Clone, Default)]
pub(crate) struct PosEval {
    lines: Vec<LineEval>,
}

/// One MultiPV line: its first move (UCI), evaluation (side-to-move's
/// perspective), and principal variation.
#[derive(Debug, Clone)]
struct LineEval {
    first_uci: Option<String>,
    score: Option<Score>,
    pv: Vec<String>,
}

impl From<&Analysis> for LineEval {
    fn from(a: &Analysis) -> Self {
        LineEval {
            first_uci: a.pv.first().cloned().or_else(|| {
                (!a.bestmove.is_empty() && a.bestmove != "(none)").then(|| a.bestmove.clone())
            }),
            score: a.score,
            pv: a.pv.clone(),
        }
    }
}

/// One reviewed ply: its evaluation (White's perspective, for the eval graph),
/// the engine's preferred move, the played move's rank, its [`Classification`]
/// and a terse factual "why" note.
#[derive(Debug, Clone, Serialize)]
pub struct MoveReview {
    /// 1-based ply index.
    pub ply: usize,
    /// Move played, normalized SAN.
    pub san: String,
    /// Evaluation after the move, **White's** perspective, in centipawns (mate
    /// clamped to ±1000); pairs with `mate` for the eval graph.
    pub eval_cp: i32,
    /// Signed mate distance from White's perspective, when the position is a
    /// forced mate (`+3` = White mates in 3, `-2` = Black does).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mate: Option<i32>,
    /// The engine's preferred move in SAN, when it differs from the one played.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub best_move: Option<String>,
    /// The engine's principal variation from the position before the move, in SAN
    /// (≤6 plies). Lets the frontend graft the whole line as a variation at a
    /// critical position. Empty when there was no line to show.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub best_line: Vec<String>,
    /// Rank of the played move among the engine's lines (1 = best), or `None`
    /// when it fell outside the searched MultiPV lines.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub played_rank: Option<u32>,
    pub classification: Classification,
    /// Rule-based factual note — never an opinion or narrative.
    pub explanation: String,
}

/// A reviewed game: the start position, every ply classified and explained, and
/// the per-side accuracy summary.
#[derive(Debug, Clone, Serialize)]
pub struct GameReview {
    pub start_fen: String,
    pub moves: Vec<MoveReview>,
    pub summary: ReviewSummary,
}

/// Review a game: replay `sans` from `start_fen`, search every position at the
/// given `depth`, and assemble the classified result. `variant` selects how
/// castling rights are parsed (Chess960 vs standard).
pub async fn review_game(
    engine: &EngineService,
    start_fen: &str,
    variant: &str,
    sans: &[String],
    depth: u32,
) -> Result<GameReview, ReviewError> {
    let mode = castling_mode(variant);
    let plies =
        position::replay(start_fen, sans, mode).map_err(|e| ReviewError::BadGame(e.to_string()))?;

    let limits = Limits::depth(depth).clamped();
    let n = plies.len();
    let mut evals: Vec<PosEval> = Vec::with_capacity(n + 1);
    // Search every position 0..=N. A move is only ever played *from* a
    // non-terminal position, so a terminal final position is left empty and
    // handled as a special case in `assemble`.
    for k in 0..=n {
        let fen = position_at(start_fen, &plies, k);
        if is_terminal(fen) {
            evals.push(PosEval::default());
            continue;
        }
        let lines = engine.analyse_multi(fen, &limits, REVIEW_MULTIPV).await?;
        evals.push(PosEval {
            lines: lines.iter().map(LineEval::from).collect(),
        });
    }

    Ok(assemble(start_fen, &plies, &evals, mode))
}

/// The FEN of position `k`: the start position at 0, else after the `k`-th move.
fn position_at<'a>(start_fen: &'a str, plies: &'a [Ply], k: usize) -> &'a str {
    if k == 0 {
        start_fen
    } else {
        &plies[k - 1].fen
    }
}

/// Whether the side to move in `fen` has no legal move (checkmate or stalemate).
fn is_terminal(fen: &str) -> bool {
    features_of_fen(fen)
        .map(|f| f.legal_move_count == 0)
        .unwrap_or(false)
}

/// Turn the gathered per-position evals into the classified review. Pure: it
/// reads positions through [`crate::position`]/[`crate::features`] but touches
/// no engine, so it is unit-testable with synthetic [`PosEval`]s.
pub(crate) fn assemble(
    start_fen: &str,
    plies: &[Ply],
    evals: &[PosEval],
    mode: CastlingMode,
) -> GameReview {
    let mut moves = Vec::with_capacity(plies.len());
    let mut costs = Vec::with_capacity(plies.len());

    for i in 1..=plies.len() {
        let fen_before = position_at(start_fen, plies, i - 1);
        let fen_after = &plies[i - 1].fen;
        let san = &plies[i - 1].san;
        let mover_white = !black_to_move(fen_before, mode).unwrap_or(false);

        let before = evals[i - 1].lines.first();
        let best_before = before
            .and_then(|l| l.score)
            .unwrap_or(Score::Cp { value: 0 });
        let best_uci = before.and_then(|l| l.first_uci.clone());
        let second_best = evals[i - 1].lines.get(1).and_then(|l| l.score);

        let played_uci = san_to_uci(fen_before, strip_suffix(san), mode).ok();
        let played_is_best = match (&played_uci, &best_uci) {
            (Some(p), Some(b)) => p == b,
            _ => false,
        };
        let played_rank = played_uci.as_ref().and_then(|p| {
            evals[i - 1]
                .lines
                .iter()
                .position(|l| l.first_uci.as_deref() == Some(p.as_str()))
                .map(|idx| idx as u32 + 1)
        });

        let after_played = after_eval(fen_after, evals.get(i), mover_white, mode);
        let classification = classify(best_before, after_played, played_is_best, second_best);

        let best_san = if played_is_best {
            None
        } else {
            best_uci
                .as_deref()
                .and_then(|u| uci_to_san(fen_before, u, mode).ok())
        };
        let best_line_san = before
            .map(|l| pv_to_san(fen_before, &l.pv, PV_SAN_PLIES, mode))
            .unwrap_or_default();
        let after_pv: Vec<String> = evals
            .get(i)
            .and_then(|e| e.lines.first())
            .map(|l| l.pv.clone())
            .unwrap_or_default();

        let fact = build_fact(FactInput {
            ply: i,
            san,
            mover_white,
            fen_before,
            fen_after,
            best_before,
            after_played,
            best_san: best_san.clone(),
            best_line_san,
            after_pv_uci: &after_pv,
            mode,
        });

        let (eval_cp, mate) = white_view(after_played, mover_white);
        moves.push(MoveReview {
            ply: i,
            san: san.clone(),
            eval_cp,
            mate,
            best_move: best_san,
            best_line: fact.best_line_san.clone(),
            played_rank,
            classification,
            explanation: explain(&fact, classification),
        });
        costs.push(MoveCost::from_eval(
            mover_white,
            best_before,
            after_played,
            classification,
        ));
    }

    GameReview {
        start_fen: start_fen.to_string(),
        moves,
        summary: summarize(&costs),
    }
}

/// The mover's-perspective evaluation *after* a move. A terminal result is read
/// straight off the board (delivered mate ⇒ a win, stalemate ⇒ a draw); a normal
/// position negates the opponent-to-move eval of `fen_after`.
fn after_eval(
    fen_after: &str,
    after: Option<&PosEval>,
    _mover_white: bool,
    _mode: CastlingMode,
) -> Score {
    if let Ok(f) = features_of_fen(fen_after) {
        if f.checkmate {
            // The move just played delivered mate: a win for the mover.
            return Score::Mate { value: 1 };
        }
        if f.stalemate || f.insufficient_material {
            return Score::Cp { value: 0 };
        }
    }
    // Opponent is to move at `fen_after`; negate to the mover's perspective.
    after
        .and_then(|e| e.lines.first())
        .and_then(|l| l.score)
        .map(negate)
        .unwrap_or(Score::Cp { value: 0 })
}

/// Convert a mover's-perspective score to White's perspective for the eval
/// graph: the clamped centipawn value and the signed mate distance (if any).
fn white_view(mover_score: Score, mover_white: bool) -> (i32, Option<i32>) {
    let white = if mover_white {
        mover_score
    } else {
        negate(mover_score)
    };
    let mate = match white {
        Score::Mate { value } => Some(value),
        Score::Cp { .. } => None,
    };
    (score_cp(white), mate)
}

/// Flip a score to the other side's perspective.
fn negate(score: Score) -> Score {
    match score {
        Score::Cp { value } => Score::Cp { value: -value },
        Score::Mate { value } => Score::Mate { value: -value },
    }
}

/// Convert a UCI principal variation to SAN, replaying from `fen` and stopping
/// at the first move that doesn't apply (mirrors the frontend PV renderer).
fn pv_to_san(fen: &str, pv_uci: &[String], max: usize, mode: CastlingMode) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = fen.to_string();
    for uci in pv_uci.iter().take(max) {
        match (
            uci_to_san(&cur, uci, mode),
            position::apply_uci(&cur, uci, mode),
        ) {
            (Ok(san), Ok((next, _))) => {
                out.push(san);
                cur = next;
            }
            _ => break,
        }
    }
    out
}

/// Strip a trailing check/mate/annotation glyph from a SAN token so it parses
/// back to a move (`Qxf7#` → `Qxf7`).
fn strip_suffix(san: &str) -> &str {
    san.trim_end_matches(['+', '#', '!', '?'])
}

/// Castling parsing mode for a PGN `[Variant]` tag.
fn castling_mode(variant: &str) -> CastlingMode {
    match variant.to_ascii_lowercase().as_str() {
        "chess960" | "fischerandom" | "fischerrandom" => CastlingMode::Chess960,
        _ => CastlingMode::Standard,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::position::STARTPOS_FEN;

    const STD: CastlingMode = CastlingMode::Standard;

    fn line(uci: &str, cp: i32, pv: &[&str]) -> LineEval {
        LineEval {
            first_uci: Some(uci.to_string()),
            score: Some(Score::Cp { value: cp }),
            pv: pv.iter().map(|s| s.to_string()).collect(),
        }
    }

    fn eval(lines: Vec<LineEval>) -> PosEval {
        PosEval { lines }
    }

    #[test]
    fn assemble_classifies_a_short_game() {
        // 1. e4 e5 2. Nf3 — replay to get the FENs, then feed synthetic evals.
        let sans: Vec<String> = ["e4", "e5", "Nf3"].iter().map(|s| s.to_string()).collect();
        let plies = position::replay(STARTPOS_FEN, &sans, STD).unwrap();

        // pos0 (start): best e4 +0.3, runner-up d4 +0.2 (White to move)
        // pos1 (after e4): best e5 +0.3 (Black to move) → mover-White after = -0.3? no:
        //   after e4 the eval is from Black's view; White's move e4 eval = -that.
        // We just need consistent numbers; pick mild evals so all are "best/good".
        let evals = vec![
            eval(vec![
                line("e2e4", 30, &["e2e4", "e7e5"]),
                line("d2d4", 20, &["d2d4"]),
            ]),
            eval(vec![
                line("e7e5", -30, &["e7e5", "g1f3"]),
                line("b8c6", -40, &["b8c6"]),
            ]),
            eval(vec![
                line("g1f3", 30, &["g1f3", "b8c6"]),
                line("b1c3", 20, &["b1c3"]),
            ]),
            eval(vec![
                line("b8c6", -30, &["b8c6"]),
                line("g8f6", -40, &["g8f6"]),
            ]),
        ];

        let review = assemble(STARTPOS_FEN, &plies, &evals, STD);
        assert_eq!(review.moves.len(), 3);
        assert_eq!(review.moves[0].san, "e4");
        assert_eq!(review.moves[0].played_rank, Some(1));
        // e4 was the engine's top move ⇒ best, no alternative move shown.
        assert_eq!(review.moves[0].classification, Classification::Best);
        assert!(review.moves[0].best_move.is_none());
        // Mild, level evals ⇒ everyone is accurate.
        assert!(review.summary.white.accuracy > 95.0);
        assert!(review.summary.black.accuracy > 95.0);
    }

    #[test]
    fn assemble_flags_a_blunder_with_the_better_move() {
        // White plays a losing move on move 1: the engine preferred e4.
        let sans: Vec<String> = ["a4"].iter().map(|s| s.to_string()).collect();
        let plies = position::replay(STARTPOS_FEN, &sans, STD).unwrap();
        let evals = vec![
            // Before: best e4 at +4.0 (a winning eval), a4 isn't a listed line.
            // The top line carries a multi-ply PV so `best_line` has a real line.
            eval(vec![
                line("e2e4", 400, &["e2e4", "e7e5", "g1f3"]),
                line("d2d4", 350, &["d2d4"]),
            ]),
            // After a4: Black to move is winning by +4.0 ⇒ White is at −4.0.
            eval(vec![line("e7e5", 400, &["e7e5"])]),
        ];
        let review = assemble(STARTPOS_FEN, &plies, &evals, STD);
        assert_eq!(review.moves.len(), 1);
        let m = &review.moves[0];
        assert_eq!(m.classification, Classification::Blunder);
        assert_eq!(m.best_move.as_deref(), Some("e4"));
        // The whole engine PV is surfaced in SAN for the frontend to graft.
        assert_eq!(m.best_line, vec!["e4", "e5", "Nf3"]);
        assert_eq!(m.played_rank, None); // a4 wasn't among the top lines
                                         // White's eval after the move is negative (losing).
        assert!(m.eval_cp < 0, "eval_cp {}", m.eval_cp);
        assert_eq!(review.summary.white.blunders, 1);
    }

    #[test]
    fn assemble_handles_a_delivered_checkmate() {
        // Scholar's mate: the final move (Qxf7#) is mate; no engine eval for the
        // terminal position, yet the move must still classify cleanly.
        let sans: Vec<String> = ["e4", "e5", "Bc4", "Nc6", "Qh5", "Nf6", "Qxf7#"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let plies = position::replay(STARTPOS_FEN, &sans, STD).unwrap();
        // Give every "before" position a single best line equal to the move
        // played (so each is "best"); the last position (after mate) is empty.
        let evals: Vec<PosEval> = vec![
            eval(vec![line("e2e4", 20, &["e2e4"])]),   // before e4
            eval(vec![line("e7e5", -20, &["e7e5"])]),  // before e5
            eval(vec![line("f1c4", 30, &["f1c4"])]),   // before Bc4
            eval(vec![line("b8c6", -30, &["b8c6"])]),  // before Nc6
            eval(vec![line("d1h5", 50, &["d1h5"])]),   // before Qh5
            eval(vec![line("g8f6", -200, &["g8f6"])]), // before Nf6 (a poor move)
            eval(vec![line("h5f7", 900, &["h5f7"])]),  // before Qxf7#
            PosEval::default(),                        // checkmated position — not searched
        ];
        let review = assemble(STARTPOS_FEN, &plies, &evals, STD);
        assert_eq!(review.moves.len(), 7);
        let mate_move = review.moves.last().unwrap();
        assert_eq!(mate_move.san, "Qxf7#");
        // Delivered mate ⇒ White's eval saturates to a winning mate score.
        assert_eq!(mate_move.mate, Some(1));
        assert_eq!(mate_move.classification, Classification::Best);
    }

    #[test]
    fn strip_suffix_drops_glyphs() {
        assert_eq!(strip_suffix("Qxf7#"), "Qxf7");
        assert_eq!(strip_suffix("Nf3+"), "Nf3");
        assert_eq!(strip_suffix("e4"), "e4");
    }

    #[test]
    fn castling_mode_maps_variant() {
        assert!(matches!(castling_mode("Chess960"), CastlingMode::Chess960));
        assert!(matches!(castling_mode("standard"), CastlingMode::Standard));
    }
}
