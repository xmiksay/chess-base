//! UCI engine integration.
//!
//! Engines (Stockfish, Lc0 + Maia weights) are external binaries spoken to over
//! the UCI protocol on stdin/stdout. The scaffold defines the engine
//! configuration model and a thin parse helper over `vampirc-uci`; the process
//! manager and analysis streaming land in the Epic 5 issues.

use std::path::PathBuf;
use vampirc_uci::{parse_one, UciMessage};

/// A configured, runnable engine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EngineConfig {
    /// Display name, e.g. "Stockfish 16" or "Maia 1100".
    pub name: String,
    /// Path to the engine binary.
    pub path: PathBuf,
    /// Optional neural-net weights file (Lc0/Maia `WeightsFile`).
    pub weights: Option<PathBuf>,
}

impl EngineConfig {
    pub fn new(name: impl Into<String>, path: impl Into<PathBuf>) -> Self {
        Self {
            name: name.into(),
            path: path.into(),
            weights: None,
        }
    }

    pub fn with_weights(mut self, weights: impl Into<PathBuf>) -> Self {
        self.weights = Some(weights.into());
        self
    }
}

/// Parse a single line of engine output into a typed UCI message.
pub fn parse_uci_line(line: &str) -> UciMessage {
    parse_one(line)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_uciok() {
        assert!(matches!(parse_uci_line("uciok"), UciMessage::UciOk));
    }

    #[test]
    fn parses_bestmove() {
        match parse_uci_line("bestmove e2e4") {
            UciMessage::BestMove { best_move, .. } => {
                assert_eq!(best_move.to_string(), "e2e4");
            }
            other => panic!("expected bestmove, got {other:?}"),
        }
    }

    #[test]
    fn weights_are_optional() {
        let sf = EngineConfig::new("Stockfish", "/usr/bin/stockfish");
        assert!(sf.weights.is_none());
        let maia =
            EngineConfig::new("Maia 1100", "/usr/bin/lc0").with_weights("/nets/maia-1100.pb");
        assert_eq!(maia.weights, Some(PathBuf::from("/nets/maia-1100.pb")));
    }
}
