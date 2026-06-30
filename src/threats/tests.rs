//! Unit tests for the static threat scan: a clean start has none, a hanging
//! piece yields one attacker→target arrow, an even trade behind a defender is
//! not a threat, and a bad FEN errors rather than panics.

use super::*;

fn arrows(shapes: &[Shape]) -> Vec<(String, Option<String>)> {
    shapes
        .iter()
        .map(|s| (s.orig.clone(), s.dest.clone()))
        .collect()
}

#[test]
fn startpos_has_no_threats() {
    let shapes = threats_standard(crate::position::STARTPOS_FEN).unwrap();
    assert!(
        shapes.is_empty(),
        "no piece is hanging in the start position"
    );
}

#[test]
fn undefended_attacked_knight_is_a_threat() {
    // Black pawn on d6 attacks e5; the white knight there is undefended.
    let fen = "4k3/8/3p4/4N3/8/8/8/4K3 w - - 0 1";
    let shapes = threats_standard(fen).unwrap();
    assert_eq!(arrows(&shapes), vec![("d6".into(), Some("e5".into()))]);
    assert_eq!(shapes[0].brush, "threat");
}

#[test]
fn pawn_attacking_a_defended_knight_still_threatens() {
    // Knight on e5 is now defended by a white pawn on d4, but a pawn (1) winning
    // a knight (3) is still a material threat.
    let fen = "4k3/8/3p4/4N3/3P4/8/8/4K3 w - - 0 1";
    let shapes = threats_standard(fen).unwrap();
    assert_eq!(arrows(&shapes), vec![("d6".into(), Some("e5".into()))]);
}

#[test]
fn defended_even_trade_is_not_a_threat() {
    // White rook a1 attacked by a black rook down the open a-file but defended by
    // the king on b1: an even rook-for-rook trade wins nothing, so no threat.
    let fen = "r3k3/8/8/8/8/8/8/RK6 w - - 0 1";
    let shapes = threats_standard(fen).unwrap();
    assert!(
        arrows(&shapes).is_empty(),
        "even, defended trade is not hanging"
    );
}

#[test]
fn undefended_even_attacker_is_a_threat() {
    // Same rook, now with the king tucked on h1 — the rook is undefended, so the
    // attack hangs it outright.
    let fen = "r3k3/8/8/8/8/8/8/R6K w - - 0 1";
    let shapes = threats_standard(fen).unwrap();
    assert_eq!(arrows(&shapes), vec![("a8".into(), Some("a1".into()))]);
}

#[test]
fn invalid_fen_errors_without_panic() {
    assert!(threats_standard("not a fen").is_err());
}
