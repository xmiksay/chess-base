//! Pure position primitives built on `shakmaty`: FEN parsing, legal-move
//! generation, and Polyglot-compatible Zobrist hashing used as the key for
//! position search.

use shakmaty::fen::Fen;
use shakmaty::san::{San, SanPlus};
use shakmaty::uci::UciMove;
use shakmaty::zobrist::Zobrist64;
use shakmaty::{Chess, EnPassantMode, Move, Position};

// Re-exported so callers select the variant without depending on `shakmaty`
// directly: `Standard` for normal chess, `Chess960` for Fischer Random (where
// castling rights reference rook files, X-FEN / Shredder-FEN). The same
// `shakmaty::Chess` type backs both (per ADR-0009).
pub use shakmaty::CastlingMode;

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

/// Parse a FEN string into a legal position under the given castling `mode`.
///
/// Use [`CastlingMode::Standard`] for normal chess and [`CastlingMode::Chess960`]
/// to accept Fischer-Random positions (castling rights as rook files).
pub fn position_from_fen(fen: &str, mode: CastlingMode) -> Result<Chess, PositionError> {
    let parsed =
        Fen::from_ascii(fen.as_bytes()).map_err(|e| PositionError::InvalidFen(e.to_string()))?;
    parsed
        .into_position::<Chess>(mode)
        .map_err(|e| PositionError::IllegalPosition(e.to_string()))
}

/// Zobrist hash (64-bit, Polyglot-compatible) of the position described by `fen`.
///
/// This is the key under which positions are indexed for "find games reaching
/// this position" search. The hash is variant-agnostic (ADR-0003); `mode` only
/// governs how the FEN's castling rights are parsed.
pub fn zobrist_of_fen(fen: &str, mode: CastlingMode) -> Result<u64, PositionError> {
    let pos = position_from_fen(fen, mode)?;
    Ok(zobrist_of_position(&pos))
}

/// Zobrist hash of an already-parsed position.
pub fn zobrist_of_position(pos: &Chess) -> u64 {
    let z: Zobrist64 = pos.zobrist_hash(EnPassantMode::Legal);
    z.0
}

/// All legal moves from `fen`, in SAN, sorted for deterministic output.
pub fn legal_sans(fen: &str, mode: CastlingMode) -> Result<Vec<String>, PositionError> {
    let pos = position_from_fen(fen, mode)?;
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
pub fn apply_san(fen: &str, san: &str, mode: CastlingMode) -> Result<(String, u64), PositionError> {
    let pos = position_from_fen(fen, mode)?;
    let mv = san_to_move(&pos, san)?;
    let next = pos.play(mv).map_err(|e| PositionError::IllegalMove {
        mv: san.to_string(),
        reason: e.to_string(),
    })?;
    Ok((fen_of_position(&next), zobrist_of_position(&next)))
}

/// Apply a single UCI move (e.g. `e2e4`, `e7e8q`) to `fen`, returning the
/// resulting FEN and Zobrist hash.
pub fn apply_uci(fen: &str, uci: &str, mode: CastlingMode) -> Result<(String, u64), PositionError> {
    let pos = position_from_fen(fen, mode)?;
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
pub fn is_legal_san(fen: &str, san: &str, mode: CastlingMode) -> Result<bool, PositionError> {
    let pos = position_from_fen(fen, mode)?;
    Ok(match San::from_ascii(san.as_bytes()) {
        Ok(parsed) => parsed.to_move(&pos).is_ok(),
        Err(_) => false,
    })
}

/// Replay a sequence of SAN moves from `start_fen`, yielding one [`Ply`] per move
/// with the FEN and Zobrist hash at each step (feeds the position index).
///
/// `start_fen` is the game's actual starting position — pass the stored start FEN
/// for games that don't begin from [`STARTPOS_FEN`] (set-up positions, Chess960).
/// `mode` selects how castling rights are parsed. Stops at the first illegal or
/// ambiguous move, propagating a `PositionError`.
pub fn replay(
    start_fen: &str,
    sans: &[impl AsRef<str>],
    mode: CastlingMode,
) -> Result<Vec<Ply>, PositionError> {
    let mut pos = position_from_fen(start_fen, mode)?;
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

    /// Alias keeping the standard-chess test calls terse.
    const STD: CastlingMode = CastlingMode::Standard;

    #[test]
    fn startpos_parses_and_has_twenty_legal_moves() {
        let sans = legal_sans(STARTPOS_FEN, STD).unwrap();
        assert_eq!(sans.len(), 20, "20 first moves in the initial position");
    }

    #[test]
    fn zobrist_is_stable_for_same_position() {
        let a = zobrist_of_fen(STARTPOS_FEN, STD).unwrap();
        let b = zobrist_of_fen(STARTPOS_FEN, STD).unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn zobrist_differs_after_a_move() {
        let after_e4 = "rnbqkbnr/pppppppp/8/8/4P3/8/PPPP1PPP/RNBQKBNR b KQkq - 0 1";
        assert_ne!(
            zobrist_of_fen(STARTPOS_FEN, STD).unwrap(),
            zobrist_of_fen(after_e4, STD).unwrap()
        );
    }

    #[test]
    fn invalid_fen_is_rejected() {
        assert!(zobrist_of_fen("not a fen", STD).is_err());
    }

    #[test]
    fn apply_san_matches_known_fen_and_zobrist() {
        // EnPassantMode::Legal omits the ep square when no capture is legal.
        let after_e4 = "rnbqkbnr/pppppppp/8/8/4P3/8/PPPP1PPP/RNBQKBNR b KQkq - 0 1";
        let (fen, zobrist) = apply_san(STARTPOS_FEN, "e4", STD).unwrap();
        assert_eq!(fen, after_e4);
        assert_eq!(zobrist, zobrist_of_fen(after_e4, STD).unwrap());
    }

    #[test]
    fn apply_uci_matches_apply_san() {
        let (san_fen, san_z) = apply_san(STARTPOS_FEN, "Nf3", STD).unwrap();
        let (uci_fen, uci_z) = apply_uci(STARTPOS_FEN, "g1f3", STD).unwrap();
        assert_eq!(san_fen, uci_fen);
        assert_eq!(san_z, uci_z);
    }

    #[test]
    fn apply_uci_handles_promotion() {
        let fen = "8/P7/8/8/8/8/8/k6K w - - 0 1";
        let (after, _) = apply_uci(fen, "a7a8q", STD).unwrap();
        assert!(
            after.starts_with("Q7/"),
            "expected a queen on a8, got {after}"
        );
    }

    #[test]
    fn illegal_san_errors_without_panic() {
        // e5 is not legal as White's first move.
        let err = apply_san(STARTPOS_FEN, "e5", STD).unwrap_err();
        assert!(matches!(err, PositionError::IllegalMove { .. }));
    }

    #[test]
    fn ambiguous_san_errors() {
        // Both knights (b3, f3) can reach d2; "Nd2" is ambiguous and must error.
        let fen = "k7/8/8/8/8/1N3N2/8/4K3 w - - 0 1";
        let err = apply_san(fen, "Nd2", STD).unwrap_err();
        assert!(matches!(err, PositionError::IllegalMove { .. }));
    }

    #[test]
    fn syntactically_invalid_san_errors() {
        let err = apply_san(STARTPOS_FEN, "zz9", STD).unwrap_err();
        assert!(matches!(err, PositionError::InvalidSan(_)));
    }

    #[test]
    fn invalid_uci_errors() {
        let err = apply_uci(STARTPOS_FEN, "nonsense", STD).unwrap_err();
        assert!(matches!(err, PositionError::InvalidUci(_)));
    }

    #[test]
    fn is_legal_san_reports_legality() {
        assert!(is_legal_san(STARTPOS_FEN, "e4", STD).unwrap());
        assert!(!is_legal_san(STARTPOS_FEN, "e5", STD).unwrap());
        // Syntactically invalid SAN is reported as not-legal, not an error.
        assert!(!is_legal_san(STARTPOS_FEN, "zz9", STD).unwrap());
        // Invalid FEN still errors.
        assert!(is_legal_san("not a fen", "e4", STD).is_err());
    }

    #[test]
    fn replay_scholars_mate_ends_in_checkmate() {
        let moves = ["e4", "e5", "Bc4", "Nc6", "Qh5", "Nf6", "Qxf7#"];
        let plies = replay(STARTPOS_FEN, &moves, STD).unwrap();
        assert_eq!(plies.len(), moves.len());
        let last = plies.last().unwrap();
        assert_eq!(last.san, "Qxf7#");
        // Each ply's Zobrist must match a fresh hash of its FEN.
        for ply in &plies {
            assert_eq!(ply.zobrist, zobrist_of_fen(&ply.fen, STD).unwrap());
        }
    }

    #[test]
    fn replay_stops_on_illegal_move() {
        let moves = ["e4", "e5", "Ke2", "Ke7", "totally-illegal"];
        assert!(replay(STARTPOS_FEN, &moves, STD).is_err());
    }

    #[test]
    fn replay_empty_yields_no_plies() {
        assert!(replay(STARTPOS_FEN, &[] as &[&str], STD)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn replay_starts_from_given_start_fen() {
        // A game beginning from a set-up position, not STARTPOS_FEN.
        let start = "4k3/8/8/8/8/8/4P3/4K3 w - - 0 1";
        let plies = replay(start, &["e4"], STD).unwrap();
        assert_eq!(plies.len(), 1);
        assert!(
            plies[0].fen.starts_with("4k3/8/8/8/4P3/8/8/4K3 b"),
            "pawn should advance from the supplied start FEN, got {}",
            plies[0].fen
        );
    }

    #[test]
    fn chess960_castling_rights_need_chess960_mode() {
        // King on e1 with the a-side rook on b1 (not a1): X-FEN keeps `KQkq`,
        // but those rights only parse under Chess960 mode.
        let fen = "1r2k2r/8/8/8/8/8/8/1R2K2R w KQkq - 0 1";
        assert!(
            position_from_fen(fen, STD).is_err(),
            "standard mode must reject castling rights with no a1/h1 rook layout"
        );
        assert!(position_from_fen(fen, CastlingMode::Chess960).is_ok());
    }

    #[test]
    fn chess960_queenside_castle_round_trips() {
        // King e1, rooks on b1/h1 (and mirrored for black). O-O-O is the a-side
        // castle: king e1->c1, rook b1->d1.
        let fen = "1r2k2r/8/8/8/8/8/8/1R2K2R w KQkq - 0 1";
        let mode = CastlingMode::Chess960;
        let (after, zobrist) = apply_san(fen, "O-O-O", mode).unwrap();
        assert!(
            after.starts_with("1r2k2r/8/8/8/8/8/8/2KR3R b"),
            "expected king on c1 and rook on d1, got {after}"
        );
        // Zobrist must be reproducible from the resulting FEN (variant-agnostic).
        assert_eq!(zobrist, zobrist_of_fen(&after, mode).unwrap());

        // The same castle expressed in UCI (king-to-rook) yields the same result.
        let (uci_after, _) = apply_uci(fen, "e1b1", mode).unwrap();
        assert_eq!(after, uci_after);
    }
}
