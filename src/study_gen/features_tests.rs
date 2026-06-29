//! Tests for [`super`] — pawn-structure & key-square classification (issue #30).
//! Every case is a curated textbook position; the assertions pin both the
//! structure tag and the key square(s) the heuristics must surface. Split out to
//! keep the module under the project's 500-line file cap.

use super::*;

/// Convenience: classify and unwrap (all FENs here are legal).
fn concepts(fen: &str) -> Concepts {
    concepts_of_fen(fen).expect("legal FEN")
}

/// Whether `key_squares` holds an entry for `square` benefiting `side`.
fn key(c: &Concepts, square: &str, side: &str) -> bool {
    c.key_squares
        .iter()
        .any(|k| k.square == square && k.side == side)
}

fn has_structure(c: &Concepts, needle: &str) -> bool {
    c.structures.iter().any(|s| s.contains(needle))
}

#[test]
fn white_iqp_blockade_square_is_d5() {
    // White's d4 is isolated (no c/e pawns); Black has no d-pawn.
    let c = concepts("rnbqkbnr/pp3ppp/2p1p3/8/3P4/8/PP3PPP/RNBQKBNR w KQkq - 0 1");
    assert!(has_structure(&c, "isolated queen's pawn (IQP) for White"));
    assert!(
        key(&c, "d5", "black"),
        "d5 is the blockade outpost for Black"
    );
    // Not mistaken for the Black-IQP or Carlsbad signatures.
    assert!(!has_structure(&c, "for Black"));
    assert!(!has_structure(&c, "Carlsbad"));
}

#[test]
fn black_iqp_blockade_square_is_d4() {
    let c = concepts("rnbqkbnr/pp3ppp/8/3p4/8/2P1P3/PP3PPP/RNBQKBNR w KQkq - 0 1");
    assert!(has_structure(&c, "isolated queen's pawn (IQP) for Black"));
    assert!(
        key(&c, "d4", "white"),
        "d4 is the blockade outpost for White"
    );
}

#[test]
fn hanging_pawns_have_two_blockade_points() {
    // White c4+d4, no b/e pawns; Black has neither a c- nor a d-pawn.
    let c = concepts("rnbqkbnr/pp3ppp/4p3/8/2PP4/8/P4PPP/RNBQKBNR w KQkq - 0 1");
    assert!(has_structure(&c, "hanging pawns (c & d) for White"));
    assert!(key(&c, "c5", "black"));
    assert!(key(&c, "d5", "black"));
    // Hanging pawns are not an IQP.
    assert!(!has_structure(&c, "isolated queen's pawn"));
}

#[test]
fn carlsbad_marks_minority_attack_break() {
    // d4 vs d5, White's c-pawn and Black's e-pawn traded, Black has a c6 pawn.
    let c = concepts("rnbqkbnr/pp3ppp/2p5/3p4/3P4/4P3/PP3PPP/RNBQKBNR w KQkq - 0 1");
    assert!(has_structure(&c, "Carlsbad"));
    assert!(key(&c, "b5", "white"), "b4-b5 is the minority-attack break");
    assert!(key(&c, "c6", "white"), "c6 is the minority-attack target");
    // The locked d-pawns mean neither side has an IQP.
    assert!(!has_structure(&c, "isolated queen's pawn"));
}

#[test]
fn hedgehog_binds_d5_and_offers_b5_break() {
    // Black a6/b6/d6/e6 wall, no c-pawn; White c4 + e4.
    let c = concepts("rnbqkbnr/5ppp/pp1pp3/8/2P1P3/8/PP3PPP/RNBQKBNR w KQkq - 0 1");
    assert!(has_structure(&c, "hedgehog (Black)"));
    assert!(key(&c, "d5", "white"), "d5 is White's bind square");
    assert!(key(&c, "b5", "black"), "...b5 is Black's freeing break");
    // The fuller hedgehog signature suppresses a bare Maroczy tag.
    assert!(!has_structure(&c, "Maroczy"));
}

#[test]
fn maroczy_bind_clamps_d5() {
    // White c4 + e4, Black has no d-pawn and not the hedgehog wall.
    let c = concepts("rnbqkbnr/pp2pppp/8/8/2P1P3/8/PP3PPP/RNBQKBNR w KQkq - 0 1");
    assert!(has_structure(&c, "Maroczy bind (White)"));
    assert!(key(&c, "d5", "white"));
    assert!(!has_structure(&c, "hedgehog"));
}

#[test]
fn white_stonewall_outpost_is_e5() {
    let c = concepts("rnbqkbnr/pppppppp/8/8/3P1P2/2P1P3/PP4PP/RNBQKBNR w KQkq - 0 1");
    assert!(has_structure(&c, "Stonewall (White)"));
    assert!(key(&c, "e5", "white"));
}

#[test]
fn french_chain_marks_both_bases() {
    // White d4/e5 against Black d5/e6 — the French advance pawn chain.
    let c = concepts("rnbqkbnr/ppp2ppp/4p3/3pP3/3P4/8/PPP2PPP/RNBQKBNR w KQkq - 0 1");
    assert!(has_structure(&c, "closed centre (French-type pawn chain)"));
    assert!(
        key(&c, "d4", "black"),
        "d4 is White's chain base, Black's target"
    );
    assert!(
        key(&c, "e6", "white"),
        "e6 is Black's chain base, White's target"
    );
}

#[test]
fn open_and_half_open_files_are_listed() {
    // White doubled c-pawns, no d-pawn; Black missing its c-pawn.
    let c = concepts("rnbqkbnr/pp1ppppp/8/8/8/2P5/PPP1PPPP/RNBQKBNR w KQkq - 0 1");
    assert!(c
        .tags
        .iter()
        .any(|t| t == "White has doubled pawns on the c-file"));
    assert!(
        c.black_half_open_files.contains(&'c'),
        "Black has no c-pawn"
    );
    assert!(
        c.white_half_open_files.contains(&'d'),
        "White has no d-pawn"
    );
}

#[test]
fn open_d_file_detected_in_maroczy() {
    let c = concepts("rnbqkbnr/pp2pppp/8/8/2P1P3/8/PP3PPP/RNBQKBNR w KQkq - 0 1");
    assert!(c.open_files.contains(&'d'), "both d-pawns are gone");
}

#[test]
fn passed_pawn_is_tagged() {
    let c = concepts("4k3/8/8/4P3/8/8/8/4K3 w - - 0 1");
    assert!(c.tags.iter().any(|t| t == "White has a passed pawn on e5"));
}

#[test]
fn backward_pawn_on_half_open_file() {
    // Black d6 pawn: its e-pawn is on e5 (advanced past it) and White's c4 pawn
    // covers the d5 stop square — a textbook backward pawn.
    let c = concepts("4k3/8/3p4/4p3/2P5/8/8/4K3 w - - 0 1");
    assert!(
        c.tags
            .iter()
            .any(|t| t == "Black has a backward pawn on d6"),
        "tags were {:?}",
        c.tags
    );
}

#[test]
fn bishop_pair_is_signalled() {
    let c = concepts("4k3/8/8/8/8/8/8/2B1KB2 w - - 0 1");
    assert!(c.tags.iter().any(|t| t == "White has the bishop pair"));
}

#[test]
fn opposite_coloured_bishops_signalled() {
    // White bishop on f1 (light), Black bishop on f8 (dark).
    let c = concepts("5bk1/8/8/8/8/8/8/5BK1 w - - 0 1");
    assert!(c.tags.iter().any(|t| t == "opposite-coloured bishops"));
}

#[test]
fn castled_king_with_full_shield_reads_safe() {
    // White castled kingside, f2/g2/h2 intact.
    let c = concepts("4k3/8/8/8/8/8/5PPP/5RK1 w - - 0 1");
    assert!(
        c.tags
            .iter()
            .any(|t| t.contains("White king on the kingside") && t.contains("intact")),
        "tags were {:?}",
        c.tags
    );
}

#[test]
fn exposed_king_on_open_file_is_flagged() {
    // White king on g1 but the g-pawn is gone and Black keeps a g-pawn → the
    // g-file is half-open straight at the king.
    let c = concepts("6k1/6p1/8/8/8/8/5P1P/5RK1 w - - 0 1");
    assert!(
        c.tags
            .iter()
            .any(|t| t.contains("White king") && t.contains("exposed")),
        "tags were {:?}",
        c.tags
    );
}

#[test]
fn startpos_has_no_structure_but_is_not_empty() {
    let c = concepts(STARTPOS_FEN);
    assert!(c.structures.is_empty());
    assert!(c.key_squares.is_empty());
    assert!(c.open_files.is_empty());
    // King-safety notes are always produced, so the concept set is non-empty.
    assert!(!c.is_empty());
}

#[test]
fn invalid_fen_errors_without_panic() {
    assert!(concepts_of_fen("not a fen").is_err());
}

use crate::position::STARTPOS_FEN;
