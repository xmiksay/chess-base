//! Pure plan trajectories built on `shakmaty`: turn an engine principal
//! variation into per-piece paths across the whole line.
//!
//! A **Plan** visualizes the *idea* behind a line. Only the side to move in the
//! start position is traced: each of its pieces that moves gets a trajectory
//! chained by square continuity — e.g. for `Nf3 … Ng5` the knight path is
//! `g1→f3→g5`, not just the next hop. Opponent replies are applied to keep the
//! board legal but are never traced.
//!
//! This is transport-agnostic domain logic (architecture.md layering rule,
//! ADR-0017): the engine WebSocket handler and the future MCP endpoint are thin
//! callers; the frontend only renders the result.

use shakmaty::uci::UciMove;
use shakmaty::{Color, Move, Position};

use crate::position::{position_from_fen, CastlingMode, PositionError};

/// Default cap on the traced side's own plies, keeping the drawn arrows readable.
pub const DEFAULT_MAX_MOVES: usize = 4;

/// One piece's path across a line: the moving piece (color-cased FEN char, e.g.
/// `'N'` / `'n'`) and the squares it visits, including its origin
/// (`["g1","f3","g5"]`).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Trajectory {
    pub piece: char,
    pub squares: Vec<String>,
}

/// The traced side's plan: one [`Trajectory`] per piece that moved.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Plan {
    pub trajectories: Vec<Trajectory>,
}

/// Trace the side-to-move's pieces through a principal variation into per-piece
/// trajectories.
///
/// `start_fen` fixes the position *and* which side is traced (its side to move).
/// `pv_uci` is the line in UCI (`["g1f3", "e7e5", "f3g5", …]`); opponent replies
/// are applied to keep the board legal but are not traced. `max_moves` caps the
/// traced side's own plies (use [`DEFAULT_MAX_MOVES`]). `mode` selects how the
/// FEN's castling rights are parsed (Standard / Chess960).
///
/// Chaining is by square continuity: a traced move whose origin equals an
/// existing trajectory's current square *extends* it; otherwise it starts a new
/// path. Captures keep the chain (the destination is still a single square).
///
/// Only an invalid `start_fen` errors. A truncated, illegal, or unparseable move
/// in `pv_uci` simply stops the trace, returning the plan traced up to that point.
pub fn plan_from_pv(
    start_fen: &str,
    pv_uci: &[String],
    max_moves: usize,
    mode: CastlingMode,
) -> Result<Plan, PositionError> {
    let mut pos = position_from_fen(start_fen, mode)?;
    let traced = pos.turn();
    let mut trajectories: Vec<Trajectory> = Vec::new();
    let mut own_plies = 0usize;

    for uci in pv_uci {
        if own_plies >= max_moves {
            break;
        }
        let mover = pos.turn();
        // A malformed or illegal PV move ends the trace without erroring.
        let Ok(parsed) = UciMove::from_ascii(uci.as_bytes()) else {
            break;
        };
        let Ok(mv) = parsed.to_move(&pos) else {
            break;
        };

        if mover == traced {
            if let Some((from, to)) = traced_squares(mv, traced) {
                chain(&mut trajectories, piece_char(mv, traced), from, to);
                own_plies += 1;
            }
        }

        // mv is validated legal; reassigning `pos` keeps the board in sync.
        let Ok(next) = pos.play(mv) else {
            break;
        };
        pos = next;
    }

    Ok(Plan { trajectories })
}

/// Origin and destination squares (as `"e2"`-style strings) for a traced move.
/// Castling reports the *king's* path (`e1→g1`), not the rook square `to()`
/// returns. Returns `None` for drops, which standard chess never produces.
fn traced_squares(mv: Move, color: Color) -> Option<(String, String)> {
    let from = mv.from()?;
    let to = match mv.castling_side() {
        Some(side) => side.king_to(color),
        None => mv.to(),
    };
    Some((from.to_string(), to.to_string()))
}

/// Color-cased FEN char of the moved piece (`'N'` for White, `'n'` for Black).
fn piece_char(mv: Move, color: Color) -> char {
    let role = mv.role();
    if color == Color::White {
        role.upper_char()
    } else {
        role.char()
    }
}

/// Extend the trajectory whose current square is `from` (same piece), else start
/// a new one. Square continuity is what chains `g1→f3` then `f3→g5`.
fn chain(trajectories: &mut Vec<Trajectory>, piece: char, from: String, to: String) {
    if let Some(t) = trajectories
        .iter_mut()
        .find(|t| t.piece == piece && t.squares.last() == Some(&from))
    {
        t.squares.push(to);
    } else {
        trajectories.push(Trajectory {
            piece,
            squares: vec![from, to],
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::position::STARTPOS_FEN;

    const STD: CastlingMode = CastlingMode::Standard;

    /// Convenience: build a PV from string literals.
    fn pv(moves: &[&str]) -> Vec<String> {
        moves.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn knight_hops_chain_into_one_path() {
        // g1f3 (White), e7e5 (Black reply, not traced), f3g5 (White).
        let plan = plan_from_pv(STARTPOS_FEN, &pv(&["g1f3", "e7e5", "f3g5"]), 4, STD).unwrap();
        assert_eq!(plan.trajectories.len(), 1);
        let t = &plan.trajectories[0];
        assert_eq!(t.piece, 'N');
        assert_eq!(t.squares, vec!["g1", "f3", "g5"]);
    }

    #[test]
    fn two_pieces_each_get_their_own_path() {
        // Knight g1->f3 and pawn e2->e4 are distinct origins → two trajectories.
        let plan = plan_from_pv(STARTPOS_FEN, &pv(&["g1f3", "e7e5", "e2e4"]), 4, STD).unwrap();
        assert_eq!(plan.trajectories.len(), 2);
        assert_eq!(plan.trajectories[0].squares, vec!["g1", "f3"]);
        assert_eq!(plan.trajectories[0].piece, 'N');
        assert_eq!(plan.trajectories[1].squares, vec!["e2", "e4"]);
        assert_eq!(plan.trajectories[1].piece, 'P');
    }

    #[test]
    fn capture_mid_path_keeps_the_chain() {
        // Knight g1->f3->e5, where f3xe5 captures a pawn: still one chained path.
        let plan =
            plan_from_pv(STARTPOS_FEN, &pv(&["g1f3", "e7e5", "f3e5", "d7d6"]), 4, STD).unwrap();
        assert_eq!(plan.trajectories.len(), 1);
        assert_eq!(plan.trajectories[0].squares, vec!["g1", "f3", "e5"]);
    }

    #[test]
    fn kingside_castle_traces_the_king_path() {
        // White to move can castle short; e1g1 must trace the king e1->g1
        // (not the rook square shakmaty's `to()` would report).
        let fen = "rnbqkbnr/pppppppp/8/8/8/5NP1/PPPPPPBP/RNBQK2R w KQkq - 0 1";
        let plan = plan_from_pv(fen, &pv(&["e1g1"]), 4, STD).unwrap();
        assert_eq!(plan.trajectories.len(), 1);
        let t = &plan.trajectories[0];
        assert_eq!(t.piece, 'K');
        assert_eq!(t.squares, vec!["e1", "g1"]);
    }

    #[test]
    fn black_to_move_traces_black_pieces() {
        let fen = "rnbqkbnr/pppppppp/8/8/4P3/8/PPPP1PPP/RNBQKBNR b KQkq - 0 1";
        // b8c6 (Black), then White's g1f3 reply is not traced, then c6d4 (Black).
        let plan = plan_from_pv(fen, &pv(&["b8c6", "g1f3", "c6d4"]), 4, STD).unwrap();
        assert_eq!(plan.trajectories.len(), 1);
        let t = &plan.trajectories[0];
        assert_eq!(t.piece, 'n', "black knight is lower-cased");
        assert_eq!(t.squares, vec!["b8", "c6", "d4"]);
    }

    #[test]
    fn max_moves_caps_the_traced_sides_plies() {
        // Four own plies available, capped at two: only g1->f3 and a separate
        // pawn push are traced (the later knight hops are dropped).
        let line = pv(&["g1f3", "e7e5", "e2e4", "d7d6", "f3g5", "h7h6"]);
        let plan = plan_from_pv(STARTPOS_FEN, &line, 2, STD).unwrap();
        let total: usize = plan.trajectories.iter().map(|t| t.squares.len() - 1).sum();
        assert_eq!(total, 2, "exactly two own plies traced");
    }

    #[test]
    fn empty_pv_yields_an_empty_plan() {
        let plan = plan_from_pv(STARTPOS_FEN, &[], 4, STD).unwrap();
        assert!(plan.trajectories.is_empty());
    }

    #[test]
    fn short_pv_traces_what_it_has() {
        let plan = plan_from_pv(STARTPOS_FEN, &pv(&["g1f3"]), 4, STD).unwrap();
        assert_eq!(plan.trajectories.len(), 1);
        assert_eq!(plan.trajectories[0].squares, vec!["g1", "f3"]);
    }

    #[test]
    fn truncated_or_illegal_pv_stops_without_erroring() {
        // g1f3 traces, then an illegal move ends the trace cleanly.
        let plan = plan_from_pv(STARTPOS_FEN, &pv(&["g1f3", "e7e5", "e2e7"]), 4, STD).unwrap();
        assert_eq!(plan.trajectories.len(), 1);
        assert_eq!(plan.trajectories[0].squares, vec!["g1", "f3"]);

        // A syntactically invalid token likewise just stops the trace.
        let plan = plan_from_pv(STARTPOS_FEN, &pv(&["g1f3", "e7e5", "zzzz"]), 4, STD).unwrap();
        assert_eq!(plan.trajectories.len(), 1);
    }

    #[test]
    fn invalid_start_fen_errors() {
        assert!(plan_from_pv("not a fen", &pv(&["g1f3"]), 4, STD).is_err());
    }
}
