//! Serializable analysis events and the pure mapping from parsed UCI messages.
//!
//! `vampirc-uci`'s [`UciMessage`] is faithful to the wire but awkward to stream
//! to a browser (durations, optionals scattered across a `Vec` of attributes).
//! [`AnalysisEvent`] is the flat, JSON-friendly shape the frontend consumes;
//! [`event_from_message`] is the I/O-free translation, unit-tested directly.

use serde::{Deserialize, Serialize};
use vampirc_uci::{UciInfoAttribute, UciMessage};

/// An engine evaluation, mirroring UCI `score cp`/`score mate`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Score {
    /// Centipawn evaluation from the side-to-move's perspective.
    Cp { value: i32 },
    /// Forced mate in this many moves (negative ⇒ side-to-move is being mated).
    Mate { value: i32 },
}

/// One `info` line distilled to the fields the UI shows. All optional because an
/// engine emits partial `info` lines (e.g. `info depth 1 currmove …`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnalysisInfo {
    pub depth: Option<u8>,
    pub seldepth: Option<u8>,
    /// 1-based principal-variation index when MultiPV > 1.
    pub multipv: Option<u16>,
    pub score: Option<Score>,
    pub nodes: Option<u64>,
    pub nps: Option<u64>,
    pub time_ms: Option<u64>,
    /// The principal variation, as UCI long-algebraic moves (`e2e4`, …).
    pub pv: Vec<String>,
}

impl AnalysisInfo {
    /// Whether this line carries anything worth streaming. Engines emit bare
    /// `info string …` / `info currmove …` chatter we drop to keep the UI quiet.
    fn is_meaningful(&self) -> bool {
        self.depth.is_some() || self.score.is_some() || !self.pv.is_empty()
    }
}

/// A streamed analysis update: either a refined `info` line or the terminal
/// `bestmove` that ends a search.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum AnalysisEvent {
    Info(AnalysisInfo),
    BestMove {
        best_move: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        ponder: Option<String>,
    },
}

/// Translate a parsed UCI message into an [`AnalysisEvent`], or `None` for
/// messages that are not part of analysis output (handshake, options, chatter).
pub fn event_from_message(msg: UciMessage) -> Option<AnalysisEvent> {
    match msg {
        UciMessage::Info(attrs) => {
            let info = info_from_attributes(&attrs);
            info.is_meaningful().then_some(AnalysisEvent::Info(info))
        }
        UciMessage::BestMove { best_move, ponder } => Some(AnalysisEvent::BestMove {
            best_move: best_move.to_string(),
            ponder: ponder.map(|m| m.to_string()),
        }),
        _ => None,
    }
}

fn info_from_attributes(attrs: &[UciInfoAttribute]) -> AnalysisInfo {
    let mut info = AnalysisInfo::default();
    for attr in attrs {
        match attr {
            UciInfoAttribute::Depth(d) => info.depth = Some(*d),
            UciInfoAttribute::SelDepth(d) => info.seldepth = Some(*d),
            UciInfoAttribute::MultiPv(n) => info.multipv = Some(*n),
            UciInfoAttribute::Nodes(n) => info.nodes = Some(*n),
            UciInfoAttribute::Nps(n) => info.nps = Some(*n),
            UciInfoAttribute::Time(d) => info.time_ms = Some(d.num_milliseconds().max(0) as u64),
            UciInfoAttribute::Score { cp, mate, .. } => {
                info.score = match (cp, mate) {
                    (_, Some(m)) => Some(Score::Mate { value: *m as i32 }),
                    (Some(c), None) => Some(Score::Cp { value: *c }),
                    (None, None) => None,
                };
            }
            UciInfoAttribute::Pv(moves) => {
                info.pv = moves.iter().map(|m| m.to_string()).collect();
            }
            _ => {}
        }
    }
    info
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::parse_uci_line;

    fn info_event(line: &str) -> AnalysisInfo {
        match event_from_message(parse_uci_line(line)) {
            Some(AnalysisEvent::Info(info)) => info,
            other => panic!("expected info event, got {other:?}"),
        }
    }

    #[test]
    fn parses_depth_score_and_pv() {
        let info =
            info_event("info depth 12 seldepth 18 score cp 34 nodes 120000 nps 600000 time 200 pv e2e4 e7e5 g1f3");
        assert_eq!(info.depth, Some(12));
        assert_eq!(info.seldepth, Some(18));
        assert_eq!(info.score, Some(Score::Cp { value: 34 }));
        assert_eq!(info.nodes, Some(120000));
        assert_eq!(info.nps, Some(600000));
        assert_eq!(info.time_ms, Some(200));
        assert_eq!(info.pv, vec!["e2e4", "e7e5", "g1f3"]);
    }

    #[test]
    fn parses_multipv_and_mate() {
        let info = info_event("info depth 10 multipv 2 score mate -3 pv f1f2 g3g2");
        assert_eq!(info.multipv, Some(2));
        assert_eq!(info.score, Some(Score::Mate { value: -3 }));
    }

    #[test]
    fn bestmove_maps_with_ponder() {
        match event_from_message(parse_uci_line("bestmove e2e4 ponder e7e5")) {
            Some(AnalysisEvent::BestMove { best_move, ponder }) => {
                assert_eq!(best_move, "e2e4");
                assert_eq!(ponder.as_deref(), Some("e7e5"));
            }
            other => panic!("expected bestmove, got {other:?}"),
        }
    }

    #[test]
    fn handshake_and_chatter_are_dropped() {
        assert!(event_from_message(parse_uci_line("uciok")).is_none());
        assert!(event_from_message(parse_uci_line("readyok")).is_none());
        // A bare info string carries no analysis data.
        assert!(
            event_from_message(parse_uci_line("info string NNUE evaluation using …")).is_none()
        );
    }

    #[test]
    fn info_event_serializes_to_tagged_json() {
        let event = AnalysisEvent::Info(AnalysisInfo {
            depth: Some(5),
            score: Some(Score::Cp { value: 12 }),
            pv: vec!["e2e4".to_string()],
            ..AnalysisInfo::default()
        });
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["type"], "info");
        assert_eq!(json["depth"], 5);
        assert_eq!(json["score"]["type"], "cp");
        assert_eq!(json["score"]["value"], 12);
        assert_eq!(json["pv"][0], "e2e4");
    }
}
