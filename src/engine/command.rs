//! Pure builders for the UCI commands we send to an engine.
//!
//! Kept I/O-free so the exact wire text is unit-testable without spawning a
//! process. The [`manager`](super::manager) module owns the actual stdin writes.

use serde::{Deserialize, Serialize};

/// Search limits for a `go` command. An all-default value (no field set) maps to
/// `go infinite` — analyse until told to `stop`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Limits {
    /// Stop after reaching this search depth (plies).
    pub depth: Option<u32>,
    /// Stop after this many milliseconds of search.
    pub movetime_ms: Option<u64>,
    /// Stop after searching this many nodes.
    pub nodes: Option<u64>,
}

impl Limits {
    /// Convenience: search to a fixed depth.
    pub fn depth(depth: u32) -> Self {
        Self {
            depth: Some(depth),
            ..Self::default()
        }
    }

    fn is_unbounded(&self) -> bool {
        self.depth.is_none() && self.movetime_ms.is_none() && self.nodes.is_none()
    }
}

/// Build the `position fen <fen>` command. The caller is responsible for passing
/// a syntactically valid FEN (the manager validates via `position.rs` first).
pub fn position_command(fen: &str) -> String {
    format!("position fen {}", fen.trim())
}

/// Build a `go` command from search limits. Unbounded limits → `go infinite`.
pub fn go_command(limits: &Limits) -> String {
    if limits.is_unbounded() {
        return "go infinite".to_string();
    }
    let mut cmd = String::from("go");
    if let Some(d) = limits.depth {
        cmd.push_str(&format!(" depth {d}"));
    }
    if let Some(ms) = limits.movetime_ms {
        cmd.push_str(&format!(" movetime {ms}"));
    }
    if let Some(n) = limits.nodes {
        cmd.push_str(&format!(" nodes {n}"));
    }
    cmd
}

/// Build a `setoption name <name> value <value>` command.
pub fn set_option_command(name: &str, value: &str) -> String {
    format!("setoption name {name} value {value}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn position_uses_fen() {
        assert_eq!(
            position_command("rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w KQkq - 0 1"),
            "position fen rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w KQkq - 0 1"
        );
    }

    #[test]
    fn empty_limits_is_infinite() {
        assert_eq!(go_command(&Limits::default()), "go infinite");
    }

    #[test]
    fn depth_limit() {
        assert_eq!(go_command(&Limits::depth(20)), "go depth 20");
    }

    #[test]
    fn combined_limits() {
        let limits = Limits {
            depth: Some(18),
            movetime_ms: Some(5000),
            nodes: Some(1_000_000),
        };
        assert_eq!(
            go_command(&limits),
            "go depth 18 movetime 5000 nodes 1000000"
        );
    }

    #[test]
    fn set_option_formats_name_and_value() {
        assert_eq!(
            set_option_command("MultiPV", "3"),
            "setoption name MultiPV value 3"
        );
    }
}
