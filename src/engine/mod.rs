//! UCI engine integration.
//!
//! Engines (Stockfish, Lc0 + Maia weights) are external binaries spoken to over
//! the UCI protocol on stdin/stdout. This module is split into:
//!
//! - [`command`] — pure builders for the UCI commands we send (`position`, `go`,
//!   `setoption`) plus the [`command::Limits`] search-limit model;
//! - [`analysis`] — pure conversion of parsed `info`/`bestmove` messages into the
//!   serializable [`analysis::AnalysisEvent`] streamed to the frontend;
//! - [`manager`] — the [`manager::Engine`] process manager: spawns a child,
//!   performs the UCI handshake, configures options, and streams analysis.
//! - [`service`] — the [`service::EngineService`] pooled facade: one engine pool
//!   behind a direct `analyse` API (batch) and the MCP `engine_analyse` tool.
//!
//! `command` and `analysis` are I/O-free and unit-tested; `manager` and
//! `service` are thin async adapters, integration-tested behind an engine-path
//! env var.

pub mod analysis;
pub mod command;
pub mod download;
pub mod manager;
pub mod registry;
pub mod service;

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use vampirc_uci::{parse_one, UciMessage};

pub use analysis::{AnalysisEvent, AnalysisInfo, Score};
pub use command::{Limits, MAX_DEPTH, MAX_MOVETIME_MS};
pub use download::{
    catalog, download_default_engines, Asset, Fetch, HttpFetcher, Manager, Plan, Platform,
};
pub use manager::Engine;
pub use registry::{resolve, EngineRegistry, RegistryError};
pub use service::{Analysis, EngineService};

/// A configured, runnable engine. `Serialize`/`Deserialize` so the
/// [`EngineRegistry`] can persist a list of these in the `settings` store.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EngineConfig {
    /// Display name, e.g. "Stockfish 16" or "Maia 1100". Also the registry key.
    pub name: String,
    /// Path to the engine binary.
    pub path: PathBuf,
    /// Optional neural-net weights file (Lc0/Maia `WeightsFile`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub weights: Option<PathBuf>,
    /// Optional launch wrapper prepended to the engine binary (a script,
    /// `wine`, a `docker exec` shim). When set, the engine is spawned as
    /// `<runner> <path> …` instead of `<path> …`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runner: Option<PathBuf>,
}

impl EngineConfig {
    pub fn new(name: impl Into<String>, path: impl Into<PathBuf>) -> Self {
        Self {
            name: name.into(),
            path: path.into(),
            weights: None,
            runner: None,
        }
    }

    pub fn with_weights(mut self, weights: impl Into<PathBuf>) -> Self {
        self.weights = Some(weights.into());
        self
    }

    pub fn with_runner(mut self, runner: impl Into<PathBuf>) -> Self {
        self.runner = Some(runner.into());
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
