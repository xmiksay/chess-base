//! Pure position primitives built on `shakmaty`: FEN parsing, legal-move
//! generation, and Polyglot-compatible Zobrist hashing used as the key for
//! position search.

use shakmaty::fen::Fen;
use shakmaty::san::San;
use shakmaty::zobrist::Zobrist64;
use shakmaty::{CastlingMode, Chess, EnPassantMode, Position};

/// Standard chess starting position in FEN.
pub const STARTPOS_FEN: &str = "rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w KQkq - 0 1";

#[derive(Debug, thiserror::Error)]
pub enum PositionError {
    #[error("invalid FEN: {0}")]
    InvalidFen(String),
    #[error("illegal position: {0}")]
    IllegalPosition(String),
}

/// Parse a FEN string into a legal standard-chess position.
pub fn position_from_fen(fen: &str) -> Result<Chess, PositionError> {
    let parsed =
        Fen::from_ascii(fen.as_bytes()).map_err(|e| PositionError::InvalidFen(e.to_string()))?;
    parsed
        .into_position::<Chess>(CastlingMode::Standard)
        .map_err(|e| PositionError::IllegalPosition(e.to_string()))
}

/// Zobrist hash (64-bit, Polyglot-compatible) of the position described by `fen`.
///
/// This is the key under which positions are indexed for "find games reaching
/// this position" search.
pub fn zobrist_of_fen(fen: &str) -> Result<u64, PositionError> {
    let pos = position_from_fen(fen)?;
    Ok(zobrist_of_position(&pos))
}

/// Zobrist hash of an already-parsed position.
pub fn zobrist_of_position(pos: &Chess) -> u64 {
    let z: Zobrist64 = pos.zobrist_hash(EnPassantMode::Legal);
    z.0
}

/// All legal moves from `fen`, in SAN, sorted for deterministic output.
pub fn legal_sans(fen: &str) -> Result<Vec<String>, PositionError> {
    let pos = position_from_fen(fen)?;
    let mut sans: Vec<String> = pos
        .legal_moves()
        .iter()
        .map(|m| San::from_move(&pos, *m).to_string())
        .collect();
    sans.sort();
    Ok(sans)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn startpos_parses_and_has_twenty_legal_moves() {
        let sans = legal_sans(STARTPOS_FEN).unwrap();
        assert_eq!(sans.len(), 20, "20 first moves in the initial position");
    }

    #[test]
    fn zobrist_is_stable_for_same_position() {
        let a = zobrist_of_fen(STARTPOS_FEN).unwrap();
        let b = zobrist_of_fen(STARTPOS_FEN).unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn zobrist_differs_after_a_move() {
        let after_e4 = "rnbqkbnr/pppppppp/8/8/4P3/8/PPPP1PPP/RNBQKBNR b KQkq - 0 1";
        assert_ne!(
            zobrist_of_fen(STARTPOS_FEN).unwrap(),
            zobrist_of_fen(after_e4).unwrap()
        );
    }

    #[test]
    fn invalid_fen_is_rejected() {
        assert!(zobrist_of_fen("not a fen").is_err());
    }
}
