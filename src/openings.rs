//! ECO / opening classification from the embedded lichess `chess-openings`
//! dataset (public domain). PGNs frequently omit the `ECO`/`Opening` tags, so we
//! classify positions ourselves: each opening's mainline is replayed once to its
//! Zobrist hash, yielding an O(1) `zobrist -> (eco, name)` lookup.
//!
//! Pure (no I/O) and unit-testable: the dataset is `include_str!`'d at compile
//! time and parsed lazily on first use.

use std::collections::HashMap;
use std::sync::OnceLock;

use crate::position::{self, CastlingMode, PositionError, STARTPOS_FEN};

/// The five lichess dataset files (`a.tsv`…`e.tsv`), one per ECO volume, embedded
/// at compile time. Each row is `eco<TAB>name<TAB>pgn`.
const DATASET: &[&str] = &[
    include_str!("../assets/eco/a.tsv"),
    include_str!("../assets/eco/b.tsv"),
    include_str!("../assets/eco/c.tsv"),
    include_str!("../assets/eco/d.tsv"),
    include_str!("../assets/eco/e.tsv"),
];

/// A classified opening: its ECO code (e.g. `B90`) and human name. Borrows
/// directly from the embedded dataset, so it is `Copy` and allocation-free.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Opening {
    /// ECO code, e.g. `B90`.
    pub eco: &'static str,
    /// Opening name, e.g. `Sicilian Defense: Najdorf Variation`.
    pub name: &'static str,
}

/// Lazily-built `zobrist -> Opening` table over the whole dataset.
fn table() -> &'static HashMap<u64, Opening> {
    static TABLE: OnceLock<HashMap<u64, Opening>> = OnceLock::new();
    TABLE.get_or_init(build_table)
}

/// Parse every embedded row, replaying its mainline to the resulting Zobrist hash.
///
/// Openings are standard chess only, so [`CastlingMode::Standard`] is used. Rows
/// that fail to parse or replay are skipped rather than panicking — a malformed
/// upstream row must not take down classification for every other position.
fn build_table() -> HashMap<u64, Opening> {
    let mut table = HashMap::new();
    for file in DATASET {
        for line in file.lines() {
            // Skip the `eco\tname\tpgn` header and any blank trailing line.
            if line.is_empty() || line.starts_with("eco\t") {
                continue;
            }
            let mut cols = line.splitn(3, '\t');
            let (Some(eco), Some(name), Some(pgn)) = (cols.next(), cols.next(), cols.next()) else {
                continue;
            };
            let Some(zobrist) = zobrist_of_pgn(pgn) else {
                continue;
            };
            table.insert(zobrist, Opening { eco, name });
        }
    }
    table
}

/// Replay a dataset `pgn` (move-numbered SAN, e.g. `1. e4 c5 2. Nf3`) from the
/// start position, returning the final position's Zobrist hash, or `None` if a
/// move is illegal/unparseable.
fn zobrist_of_pgn(pgn: &str) -> Option<u64> {
    // Drop move-number tokens (`1.`, `12.`, `3...`): SAN never starts with a digit.
    let sans: Vec<&str> = pgn
        .split_whitespace()
        .filter(|tok| !tok.starts_with(|c: char| c.is_ascii_digit()))
        .collect();
    let plies = position::replay(STARTPOS_FEN, &sans, CastlingMode::Standard).ok()?;
    plies.last().map(|p| p.zobrist)
}

/// Opening at the position with this Zobrist hash, if it names a known opening.
/// O(1) on the key (ADR-0003: the same hash the position index uses).
pub fn opening_of_zobrist(zobrist: u64) -> Option<Opening> {
    table().get(&zobrist).copied()
}

/// Opening for the position described by `fen` (standard-chess castling rules).
///
/// Returns `Ok(None)` when the position matches no known opening; errors only on
/// an unparseable FEN.
pub fn eco_of_position(fen: &str) -> Result<Option<Opening>, PositionError> {
    let zobrist = position::zobrist_of_fen(fen, CastlingMode::Standard)?;
    Ok(opening_of_zobrist(zobrist))
}

/// Classify a game by the **longest** matching opening along its mainline: replay
/// the SAN moves and return the opening at the deepest ply that names one.
///
/// This is what ingest uses to assign an ECO even when the PGN carries no tag.
/// `start_fen` is the game's actual starting position (use [`STARTPOS_FEN`] for
/// normal games). The deepest matching position wins, so a specific line (e.g.
/// the Najdorf) is preferred over the bare opening it transposes from.
pub fn classify_mainline(
    start_fen: &str,
    sans: &[impl AsRef<str>],
    mode: CastlingMode,
) -> Result<Option<Opening>, PositionError> {
    let plies = position::replay(start_fen, sans, mode)?;
    Ok(plies
        .iter()
        .rev()
        .find_map(|ply| opening_of_zobrist(ply.zobrist)))
}

#[cfg(test)]
mod tests {
    use super::*;

    const STD: CastlingMode = CastlingMode::Standard;

    #[test]
    fn dataset_loads_thousands_of_openings() {
        // Sanity: the embedded lichess dataset has a few thousand rows.
        assert!(
            table().len() > 3000,
            "expected the full dataset, got {}",
            table().len()
        );
    }

    #[test]
    fn classifies_known_opening_by_fen() {
        // 1. e4 c5 — the Sicilian Defense.
        let after = "rnbqkbnr/pp1ppppp/8/2p5/4P3/8/PPPP1PPP/RNBQKBNR w KQkq - 0 2";
        let op = eco_of_position(after)
            .unwrap()
            .expect("Sicilian is a known opening");
        assert_eq!(op.eco, "B20");
        assert!(op.name.starts_with("Sicilian"), "got {}", op.name);
    }

    #[test]
    fn classifies_ruy_lopez() {
        // 1. e4 e5 2. Nf3 Nc6 3. Bb5 — the Ruy Lopez (C60+).
        let op = classify_mainline(STARTPOS_FEN, &["e4", "e5", "Nf3", "Nc6", "Bb5"], STD)
            .unwrap()
            .expect("Ruy Lopez is a known opening");
        assert!(op.eco.starts_with('C'), "got {}", op.eco);
        assert!(op.name.starts_with("Ruy Lopez"), "got {}", op.name);
    }

    #[test]
    fn longest_match_wins_over_shorter_prefix() {
        // The Najdorf is a deeper, more specific line than the bare Sicilian; the
        // deepest matching ply must win.
        let najdorf = [
            "e4", "c5", "Nf3", "d6", "d4", "cxd4", "Nxd4", "Nf6", "Nc3", "a6",
        ];
        let op = classify_mainline(STARTPOS_FEN, &najdorf, STD)
            .unwrap()
            .expect("Najdorf is a known opening");
        assert!(
            op.name.contains("Najdorf"),
            "expected the deepest line, got {}",
            op.name
        );
    }

    #[test]
    fn unknown_position_classifies_to_none() {
        // A late middlegame/endgame position names no opening.
        let endgame = "8/8/8/4k3/8/4K3/4P3/8 w - - 0 1";
        assert_eq!(eco_of_position(endgame).unwrap(), None);
    }

    #[test]
    fn startpos_is_not_an_opening() {
        // No move has been played; classification yields nothing.
        assert_eq!(eco_of_position(STARTPOS_FEN).unwrap(), None);
        assert_eq!(
            classify_mainline(STARTPOS_FEN, &[] as &[&str], STD).unwrap(),
            None
        );
    }

    #[test]
    fn lookup_is_consistent_between_fen_and_mainline() {
        // The two entry points must agree on the same position.
        let sans = ["d4", "Nf6", "c4", "g6"];
        let by_mainline = classify_mainline(STARTPOS_FEN, &sans, STD).unwrap();
        let plies = position::replay(STARTPOS_FEN, &sans, STD).unwrap();
        let by_fen = eco_of_position(&plies.last().unwrap().fen).unwrap();
        assert_eq!(by_mainline, by_fen);
        assert!(
            by_fen.is_some(),
            "King's Indian/Grünfeld setup should be known"
        );
    }

    #[test]
    fn invalid_fen_errors() {
        assert!(eco_of_position("not a fen").is_err());
    }
}
