//! Pure position **feature tags**: factual, I/O-free descriptors of a single
//! position derived straight from the board (material, game phase, side to move,
//! check/mate state, castling rights, mobility).
//!
//! These are *grounded facts*, not strategic judgements — the interactive
//! analysis tool (issue #33) hands them to the model alongside engine eval and
//! DB stats so an "explain this position" answer cites tool output rather than
//! hallucinating. The deeper pawn-structure / key-square classification (issue
//! #30) layers onto this same module without changing its callers.
//!
//! Transport-agnostic per the architecture layering rule: like `position` and
//! `plans`, this is fully unit-testable with no DB / engine / network.

use serde::Serialize;
use shakmaty::{CastlingSide, Color, Position, Role};

use crate::position::{position_from_fen, CastlingMode, PositionError};

/// Centipawn-free piece values (in pawns) used for the material balance.
fn piece_points(role: Role) -> u32 {
    match role {
        Role::Pawn => 1,
        Role::Knight | Role::Bishop => 3,
        Role::Rook => 5,
        Role::Queen => 9,
        Role::King => 0,
    }
}

/// Weight a piece contributes to the **game phase** measure (kings/pawns: 0).
/// Both armies intact sum to 24; the count shrinks toward the endgame.
fn phase_weight(role: Role) -> u32 {
    match role {
        Role::Knight | Role::Bishop => 1,
        Role::Rook => 2,
        Role::Queen => 4,
        _ => 0,
    }
}

/// One side's piece census plus its total value (in pawns, king excluded).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct SideMaterial {
    pub pawns: u8,
    pub knights: u8,
    pub bishops: u8,
    pub rooks: u8,
    pub queens: u8,
    /// Total value in pawns (P=1, N=B=3, R=5, Q=9), excluding the king.
    pub points: u32,
}

/// Castling rights still available to each side, per king/queen side.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct CastlingRights {
    pub white_kingside: bool,
    pub white_queenside: bool,
    pub black_kingside: bool,
    pub black_queenside: bool,
}

/// The factual feature set for a position. Every field is computed from the
/// board — nothing here is an opinion the model could not verify itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Features {
    pub fen: String,
    /// `"white"` or `"black"`.
    pub side_to_move: String,
    pub fullmove_number: u32,
    /// `"opening"`, `"middlegame"` or `"endgame"` (by remaining material).
    pub phase: String,
    pub white: SideMaterial,
    pub black: SideMaterial,
    /// White points minus Black points (positive ⇒ White is materially ahead).
    pub material_balance: i32,
    pub in_check: bool,
    pub checkmate: bool,
    pub stalemate: bool,
    pub insufficient_material: bool,
    /// Legal moves for the side to move (0 ⇒ mate or stalemate).
    pub legal_move_count: usize,
    pub castling: CastlingRights,
    /// Short human-readable descriptors summarising the above for quick grounding.
    pub tags: Vec<String>,
}

/// Extract the [`Features`] of the position described by `fen` (standard chess).
///
/// Errors only on an invalid / illegal FEN, propagated as a [`PositionError`];
/// every other field is total over a legal position.
pub fn features_of_fen(fen: &str) -> Result<Features, PositionError> {
    let pos = position_from_fen(fen, CastlingMode::Standard)?;
    let board = pos.board();

    let side = |color: Color| {
        let m = board.material_side(color);
        let points = Role::ALL
            .into_iter()
            .map(|r| u32::from(*m.get(r)) * piece_points(r))
            .sum();
        SideMaterial {
            pawns: m.pawn,
            knights: m.knight,
            bishops: m.bishop,
            rooks: m.rook,
            queens: m.queen,
            points,
        }
    };
    let white = side(Color::White);
    let black = side(Color::Black);
    let material_balance = white.points as i32 - black.points as i32;

    let phase_value: u32 = Role::ALL
        .into_iter()
        .map(|r| board.by_role(r).count() as u32 * phase_weight(r))
        .sum();
    let fullmove_number = pos.fullmoves().get();
    let phase = classify_phase(phase_value, fullmove_number);

    let in_check = pos.is_check();
    let checkmate = pos.is_checkmate();
    let stalemate = pos.is_stalemate();
    let insufficient_material = pos.is_insufficient_material();
    let legal_move_count = pos.legal_moves().len();

    let castles = pos.castles();
    let castling = CastlingRights {
        white_kingside: castles.has(Color::White, CastlingSide::KingSide),
        white_queenside: castles.has(Color::White, CastlingSide::QueenSide),
        black_kingside: castles.has(Color::Black, CastlingSide::KingSide),
        black_queenside: castles.has(Color::Black, CastlingSide::QueenSide),
    };

    let side_to_move = color_name(pos.turn()).to_string();
    let tags = build_tags(
        pos.turn(),
        &phase,
        material_balance,
        checkmate,
        stalemate,
        in_check,
        insufficient_material,
        white.queens == 0 && black.queens == 0,
    );

    Ok(Features {
        fen: fen.to_string(),
        side_to_move,
        fullmove_number,
        phase,
        white,
        black,
        material_balance,
        in_check,
        checkmate,
        stalemate,
        insufficient_material,
        legal_move_count,
        castling,
        tags,
    })
}

/// Classify the game phase from the remaining phase material (0..=24) and the
/// move number: few pieces ⇒ endgame; a full board early ⇒ opening.
fn classify_phase(phase_value: u32, fullmove_number: u32) -> String {
    if phase_value <= 6 {
        "endgame"
    } else if phase_value >= 20 && fullmove_number <= 10 {
        "opening"
    } else {
        "middlegame"
    }
    .to_string()
}

fn color_name(color: Color) -> &'static str {
    match color {
        Color::White => "white",
        Color::Black => "black",
    }
}

/// Assemble the short factual tag list. Mate/stalemate/check are mutually
/// exclusive enough that we emit the strongest single state.
#[allow(clippy::too_many_arguments)]
fn build_tags(
    turn: Color,
    phase: &str,
    material_balance: i32,
    checkmate: bool,
    stalemate: bool,
    in_check: bool,
    insufficient_material: bool,
    queens_off: bool,
) -> Vec<String> {
    let mut tags = vec![
        format!("{} to move", capitalize(color_name(turn))),
        phase.to_string(),
    ];

    if checkmate {
        tags.push("checkmate".to_string());
    } else if stalemate {
        tags.push("stalemate".to_string());
    } else if in_check {
        tags.push(format!("{} king in check", color_name(turn)));
    }

    if material_balance != 0 {
        let leader = if material_balance > 0 {
            "White"
        } else {
            "Black"
        };
        let points = material_balance.abs();
        let unit = if points == 1 { "point" } else { "points" };
        tags.push(format!("{leader} is up {points} {unit} of material"));
    } else {
        tags.push("material is level".to_string());
    }

    if queens_off {
        tags.push("queens off the board".to_string());
    }
    if insufficient_material {
        tags.push("insufficient mating material".to_string());
    }
    tags
}

/// Uppercase the first ASCII letter (`"white"` ⇒ `"White"`).
fn capitalize(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(first) => first.to_ascii_uppercase().to_string() + chars.as_str(),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::position::STARTPOS_FEN;

    #[test]
    fn startpos_is_opening_level_and_full_material() {
        let f = features_of_fen(STARTPOS_FEN).unwrap();
        assert_eq!(f.side_to_move, "white");
        assert_eq!(f.phase, "opening");
        assert_eq!(f.fullmove_number, 1);
        assert_eq!(f.material_balance, 0);
        assert_eq!(f.white.points, 39); // 8+6+6+10+9
        assert_eq!(f.black.points, 39);
        assert_eq!(f.legal_move_count, 20);
        assert!(f.castling.white_kingside && f.castling.black_queenside);
        assert!(f.tags.contains(&"White to move".to_string()));
        assert!(f.tags.contains(&"material is level".to_string()));
        assert!(!f.in_check && !f.checkmate);
    }

    #[test]
    fn material_imbalance_is_signed_and_tagged() {
        // White is a full rook up (Black has no a8 rook).
        let fen = "1nbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w KQk - 0 1";
        let f = features_of_fen(fen).unwrap();
        assert_eq!(f.material_balance, 5);
        assert!(f
            .tags
            .iter()
            .any(|t| t == "White is up 5 points of material"));
        // Black lost its kingside castling? No — only its a8 rook; queenside gone.
        assert!(!f.castling.black_queenside);
        assert!(f.castling.black_kingside);
    }

    #[test]
    fn checkmate_is_detected_and_tagged() {
        // Fool's mate: Black has just played Qh4#.
        let fen = "rnb1kbnr/pppp1ppp/8/4p3/6Pq/5P2/PPPPP2P/RNBQKBNR w KQkq - 1 3";
        let f = features_of_fen(fen).unwrap();
        assert!(f.checkmate);
        assert!(f.in_check);
        assert_eq!(f.legal_move_count, 0);
        assert!(f.tags.contains(&"checkmate".to_string()));
    }

    #[test]
    fn king_and_pawn_endgame_is_classified_endgame() {
        let fen = "8/8/4k3/8/8/4K3/4P3/8 w - - 0 1";
        let f = features_of_fen(fen).unwrap();
        assert_eq!(f.phase, "endgame");
        assert!(f.tags.contains(&"queens off the board".to_string()));
        // Lone kings + a pawn: not insufficient (the pawn can promote).
        assert!(!f.insufficient_material);
    }

    #[test]
    fn lone_kings_are_insufficient_material() {
        let f = features_of_fen("8/8/4k3/8/8/4K3/8/8 w - - 0 1").unwrap();
        assert!(f.insufficient_material);
        assert!(f.tags.contains(&"insufficient mating material".to_string()));
    }

    #[test]
    fn stalemate_is_distinguished_from_check() {
        // Classic stalemate: Black to move, king on a8, not in check, no moves.
        let fen = "k7/2Q5/2K5/8/8/8/8/8 b - - 0 1";
        let f = features_of_fen(fen).unwrap();
        assert!(f.stalemate);
        assert!(!f.in_check);
        assert_eq!(f.legal_move_count, 0);
        assert!(f.tags.contains(&"stalemate".to_string()));
    }

    #[test]
    fn invalid_fen_errors_without_panic() {
        assert!(features_of_fen("not a fen").is_err());
    }
}
