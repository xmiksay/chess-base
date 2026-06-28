//! Pure position primitives built on `shakmaty`: FEN parsing, legal-move
//! generation, and Polyglot-compatible Zobrist hashing used as the key for
//! position search.

use shakmaty::fen::Fen;
use shakmaty::san::{San, SanPlus};
use shakmaty::uci::UciMove;
use shakmaty::zobrist::Zobrist64;
use shakmaty::{CastlingMode, Chess, EnPassantMode, Move, Position};

/// Standard chess starting position in FEN.
pub const STARTPOS_FEN: &str = "rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w KQkq - 0 1";

#[derive(Debug, thiserror::Error)]
pub enum PositionError {
    #[error("invalid FEN: {0}")]
    InvalidFen(String),
    #[error("illegal position: {0}")]
    IllegalPosition(String),
    #[error("invalid SAN '{0}'")]
    InvalidSan(String),
    #[error("invalid UCI '{0}'")]
    InvalidUci(String),
    #[error("illegal move '{mv}': {reason}")]
    IllegalMove { mv: String, reason: String },
}

/// One ply of a replayed game: the move that was played (in normalized SAN) and
/// the resulting position's FEN and Zobrist hash. The Zobrist feeds the position
/// index ("find games reaching this position").
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ply {
    /// Move played, in normalized SAN (with check/mate suffix, e.g. `Qxf7#`).
    pub san: String,
    /// FEN of the position *after* the move.
    pub fen: String,
    /// Zobrist hash of the position after the move.
    pub zobrist: u64,
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

/// FEN of an already-parsed position.
pub fn fen_of_position(pos: &Chess) -> String {
    Fen::from_position(pos, EnPassantMode::Legal).to_string()
}

/// Parse a SAN string against `pos` into a concrete legal move.
///
/// Errors on syntactically invalid SAN (`InvalidSan`) or SAN that is illegal /
/// ambiguous in this position (`IllegalMove`).
fn san_to_move(pos: &Chess, san: &str) -> Result<Move, PositionError> {
    let parsed =
        San::from_ascii(san.as_bytes()).map_err(|_| PositionError::InvalidSan(san.to_string()))?;
    parsed.to_move(pos).map_err(|e| PositionError::IllegalMove {
        mv: san.to_string(),
        reason: e.to_string(),
    })
}

/// Apply a single SAN move to `fen`, returning the resulting FEN and Zobrist hash.
pub fn apply_san(fen: &str, san: &str) -> Result<(String, u64), PositionError> {
    let pos = position_from_fen(fen)?;
    let mv = san_to_move(&pos, san)?;
    let next = pos.play(mv).map_err(|e| PositionError::IllegalMove {
        mv: san.to_string(),
        reason: e.to_string(),
    })?;
    Ok((fen_of_position(&next), zobrist_of_position(&next)))
}

/// Apply a single UCI move (e.g. `e2e4`, `e7e8q`) to `fen`, returning the
/// resulting FEN and Zobrist hash.
pub fn apply_uci(fen: &str, uci: &str) -> Result<(String, u64), PositionError> {
    let pos = position_from_fen(fen)?;
    let parsed = UciMove::from_ascii(uci.as_bytes())
        .map_err(|_| PositionError::InvalidUci(uci.to_string()))?;
    let mv = parsed
        .to_move(&pos)
        .map_err(|e| PositionError::IllegalMove {
            mv: uci.to_string(),
            reason: e.to_string(),
        })?;
    let next = pos.play(mv).map_err(|e| PositionError::IllegalMove {
        mv: uci.to_string(),
        reason: e.to_string(),
    })?;
    Ok((fen_of_position(&next), zobrist_of_position(&next)))
}

/// Whether `san` is a legal move in the position described by `fen`.
///
/// Reused by studies and MCP tools to validate user input. Invalid FEN still
/// errors; syntactically invalid or illegal SAN simply yields `Ok(false)`.
pub fn is_legal_san(fen: &str, san: &str) -> Result<bool, PositionError> {
    let pos = position_from_fen(fen)?;
    Ok(match San::from_ascii(san.as_bytes()) {
        Ok(parsed) => parsed.to_move(&pos).is_ok(),
        Err(_) => false,
    })
}

/// Replay a sequence of SAN moves from `start_fen`, yielding one [`Ply`] per move
/// with the FEN and Zobrist hash at each step (feeds the position index).
///
/// Stops at the first illegal or ambiguous move, propagating a `PositionError`.
pub fn replay(start_fen: &str, sans: &[impl AsRef<str>]) -> Result<Vec<Ply>, PositionError> {
    let mut pos = position_from_fen(start_fen)?;
    let mut plies = Vec::with_capacity(sans.len());
    for san in sans {
        let san = san.as_ref();
        let mv = san_to_move(&pos, san)?;
        // mv is already validated as legal, so play unchecked; this also yields
        // normalized SAN with the check/mate suffix (e.g. `Qxf7#`).
        let normalized = SanPlus::from_move_and_play_unchecked(&mut pos, mv).to_string();
        plies.push(Ply {
            san: normalized,
            fen: fen_of_position(&pos),
            zobrist: zobrist_of_position(&pos),
        });
    }
    Ok(plies)
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

    #[test]
    fn apply_san_matches_known_fen_and_zobrist() {
        // EnPassantMode::Legal omits the ep square when no capture is legal.
        let after_e4 = "rnbqkbnr/pppppppp/8/8/4P3/8/PPPP1PPP/RNBQKBNR b KQkq - 0 1";
        let (fen, zobrist) = apply_san(STARTPOS_FEN, "e4").unwrap();
        assert_eq!(fen, after_e4);
        assert_eq!(zobrist, zobrist_of_fen(after_e4).unwrap());
    }

    #[test]
    fn apply_uci_matches_apply_san() {
        let (san_fen, san_z) = apply_san(STARTPOS_FEN, "Nf3").unwrap();
        let (uci_fen, uci_z) = apply_uci(STARTPOS_FEN, "g1f3").unwrap();
        assert_eq!(san_fen, uci_fen);
        assert_eq!(san_z, uci_z);
    }

    #[test]
    fn apply_uci_handles_promotion() {
        let fen = "8/P7/8/8/8/8/8/k6K w - - 0 1";
        let (after, _) = apply_uci(fen, "a7a8q").unwrap();
        assert!(
            after.starts_with("Q7/"),
            "expected a queen on a8, got {after}"
        );
    }

    #[test]
    fn illegal_san_errors_without_panic() {
        // e5 is not legal as White's first move.
        let err = apply_san(STARTPOS_FEN, "e5").unwrap_err();
        assert!(matches!(err, PositionError::IllegalMove { .. }));
    }

    #[test]
    fn ambiguous_san_errors() {
        // Both knights (b3, f3) can reach d2; "Nd2" is ambiguous and must error.
        let fen = "k7/8/8/8/8/1N3N2/8/4K3 w - - 0 1";
        let err = apply_san(fen, "Nd2").unwrap_err();
        assert!(matches!(err, PositionError::IllegalMove { .. }));
    }

    #[test]
    fn syntactically_invalid_san_errors() {
        let err = apply_san(STARTPOS_FEN, "zz9").unwrap_err();
        assert!(matches!(err, PositionError::InvalidSan(_)));
    }

    #[test]
    fn invalid_uci_errors() {
        let err = apply_uci(STARTPOS_FEN, "nonsense").unwrap_err();
        assert!(matches!(err, PositionError::InvalidUci(_)));
    }

    #[test]
    fn is_legal_san_reports_legality() {
        assert!(is_legal_san(STARTPOS_FEN, "e4").unwrap());
        assert!(!is_legal_san(STARTPOS_FEN, "e5").unwrap());
        // Syntactically invalid SAN is reported as not-legal, not an error.
        assert!(!is_legal_san(STARTPOS_FEN, "zz9").unwrap());
        // Invalid FEN still errors.
        assert!(is_legal_san("not a fen", "e4").is_err());
    }

    #[test]
    fn replay_scholars_mate_ends_in_checkmate() {
        let moves = ["e4", "e5", "Bc4", "Nc6", "Qh5", "Nf6", "Qxf7#"];
        let plies = replay(STARTPOS_FEN, &moves).unwrap();
        assert_eq!(plies.len(), moves.len());
        let last = plies.last().unwrap();
        assert_eq!(last.san, "Qxf7#");
        // Each ply's Zobrist must match a fresh hash of its FEN.
        for ply in &plies {
            assert_eq!(ply.zobrist, zobrist_of_fen(&ply.fen).unwrap());
        }
    }

    #[test]
    fn replay_stops_on_illegal_move() {
        let moves = ["e4", "e5", "Ke2", "Ke7", "totally-illegal"];
        assert!(replay(STARTPOS_FEN, &moves).is_err());
    }

    #[test]
    fn replay_empty_yields_no_plies() {
        assert!(replay(STARTPOS_FEN, &[] as &[&str]).unwrap().is_empty());
    }
}
