//! Pure threat detection for the Threats board overlay (issue #123).
//!
//! Given a position, surface which of the side-to-move's pieces are *hanging* —
//! attacked by the opponent and either undefended, or defended only behind a
//! cheaper attacker (so a capture wins material). Each is reported as a red
//! arrow from the cheapest attacker to the threatened piece, reusing the shared
//! [`Shape`] model so it renders straight to the board like Plans (#60) and
//! pinned study shapes (#61).
//!
//! This is a cheap static scan (no engine search): it ignores pins, X-rays and
//! deeper tactics by design, trading completeness for a fast, deterministic,
//! I/O-free overlay that is fully unit-testable. The HTTP surface lives in
//! [`routes`].

use shakmaty::{Position, Role};

use crate::pgn_tree::Shape;
use crate::position::{position_from_fen, CastlingMode, PositionError};

/// chessground brush key for threat arrows; the frontend registers it red.
const THREAT_BRUSH: &str = "threat";

/// Relative piece value used to decide whether a capture wins material. The king
/// is never a material target, so it is excluded from the scan entirely.
fn role_value(role: Role) -> u32 {
    match role {
        Role::Pawn => 1,
        Role::Knight | Role::Bishop => 3,
        Role::Rook => 5,
        Role::Queen => 9,
        Role::King => u32::MAX,
    }
}

/// Threatened pieces of the side to move in `fen`, as red arrows from the
/// cheapest attacker to each hanging piece. Sorted by `(orig, dest)` square for
/// deterministic output. Errors only on an unparseable / illegal FEN.
pub fn threats(fen: &str, mode: CastlingMode) -> Result<Vec<Shape>, PositionError> {
    let pos = position_from_fen(fen, mode)?;
    let board = pos.board();
    let us = pos.turn();
    let them = us.other();
    let occupied = board.occupied();

    let mut shapes = Vec::new();
    for sq in board.by_color(us) {
        let role = match board.role_at(sq) {
            Some(r) if r != Role::King => r,
            _ => continue,
        };

        // Cheapest enemy piece attacking this square, if any.
        let cheapest_attacker = board
            .attacks_to(sq, them, occupied)
            .into_iter()
            .filter_map(|a| board.role_at(a).map(|r| (role_value(r), a)))
            .min();
        let (attacker_value, attacker_sq) = match cheapest_attacker {
            Some(found) => found,
            None => continue,
        };

        // Hanging if undefended, or if the cheapest attacker is worth less than
        // the piece (an even or favourable trade for the opponent).
        let defended = board.attacks_to(sq, us, occupied).any();
        if !defended || attacker_value < role_value(role) {
            shapes.push(Shape {
                orig: attacker_sq.to_string(),
                dest: Some(sq.to_string()),
                brush: THREAT_BRUSH.to_string(),
            });
        }
    }

    shapes.sort_by(|a, b| (&a.orig, &a.dest).cmp(&(&b.orig, &b.dest)));
    Ok(shapes)
}

/// Convenience: standard-chess threats (the common case for the analysis board).
pub fn threats_standard(fen: &str) -> Result<Vec<Shape>, PositionError> {
    threats(fen, CastlingMode::Standard)
}

pub mod routes;

#[cfg(test)]
mod tests;
