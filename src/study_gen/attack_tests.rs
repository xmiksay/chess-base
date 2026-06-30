//! Unit tests for the pure pawn-storm detector (#142). All cases are hand-built
//! FENs + UCI lines — no engine, no DB.

use super::*;

const STD: CastlingMode = CastlingMode::Standard;

/// Convenience: build a PV from string literals.
fn pv(moves: &[&str]) -> Vec<String> {
    moves.iter().map(|s| s.to_string()).collect()
}

#[test]
fn two_pawn_pushes_toward_the_king_is_a_storm() {
    // White king e1, Black king g8; White marches the h-pawn h2→h4→h5 while
    // Black only shuffles the king. Two pushes one file from the king ⇒ storm.
    let fen = "6k1/8/8/8/8/8/6PP/4K3 w - - 0 1";
    let line = pv(&["h2h4", "g8f8", "h4h5"]);
    let signal = pawn_storm(fen, &line, STD, &AttackConfig::default())
        .unwrap()
        .expect("h-pawn storm toward g8");
    assert_eq!(signal.pawn, 'P');
    assert_eq!(signal.path, vec!["h2", "h4", "h5"]);
    assert_eq!(signal.advances, 2);
}

#[test]
fn black_pawn_storm_is_lower_cased() {
    // Mirror image: Black storms the g-pawn at White's king on g1.
    let fen = "4k3/6pp/8/8/8/8/8/6K1 b - - 0 1";
    let line = pv(&["g7g5", "g1h1", "g5g4"]);
    let signal = pawn_storm(fen, &line, STD, &AttackConfig::default())
        .unwrap()
        .expect("g-pawn storm toward g1");
    assert_eq!(signal.pawn, 'p', "black pawn is lower-cased");
    assert_eq!(signal.path, vec!["g7", "g5", "g4"]);
    assert_eq!(signal.advances, 2);
}

#[test]
fn a_single_push_is_below_the_storm_threshold() {
    // h2-h4 advances two ranks but is only one push — not yet a storm.
    let fen = "6k1/8/8/8/8/8/6PP/4K3 w - - 0 1";
    let signal = pawn_storm(fen, &pv(&["h2h4"]), STD, &AttackConfig::default()).unwrap();
    assert!(signal.is_none());
}

#[test]
fn pushes_away_from_the_king_are_not_a_storm() {
    // Same two h-pawn pushes, but the enemy king is on a8 — seven files away.
    let fen = "k7/8/8/8/8/8/6PP/4K3 w - - 0 1";
    let line = pv(&["h2h4", "a8b8", "h4h5"]);
    let signal = pawn_storm(fen, &line, STD, &AttackConfig::default()).unwrap();
    assert!(signal.is_none(), "h-file storm is far from an a-file king");
}

#[test]
fn widening_the_king_zone_admits_a_farther_pawn() {
    // A king five files away: out of the default zone, in once it is widened.
    let fen = "k7/8/8/8/8/8/6PP/4K3 w - - 0 1";
    let line = pv(&["h2h4", "a8b8", "h4h5"]);
    let wide = AttackConfig {
        king_zone_files: 7,
        ..AttackConfig::default()
    };
    assert!(pawn_storm(fen, &line, STD, &wide).unwrap().is_some());
}

#[test]
fn non_pawn_advances_are_ignored() {
    // A knight hopping toward the king is not a pawn storm.
    let fen = "6k1/8/8/8/8/8/6P1/4K1N1 w - - 0 1";
    let line = pv(&["g1f3", "g8f8", "f3h4"]);
    let signal = pawn_storm(fen, &line, STD, &AttackConfig::default()).unwrap();
    assert!(signal.is_none());
}

#[test]
fn invalid_start_fen_errors() {
    assert!(pawn_storm("not a fen", &pv(&["h2h4"]), STD, &AttackConfig::default()).is_err());
}
