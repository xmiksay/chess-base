//! Mode A — fast, engine-only full-game review (issue #119).
//!
//! One click on a stored game walks the engine over every ply, classifies each
//! move (best / great / good / inaccuracy / mistake / blunder) and writes a
//! short, *rule-based* "why" note — eval swing, the better move, material won or
//! lost, a missed or allowed mate — all mechanically derived from engine output,
//! no LLM and no API key. This is the tactical, per-move counterpart to the
//! LLM study generator (Mode B), and the two share one seam: the
//! [`MoveFact`](explain::MoveFact) struct, which Mode A renders as a terse note
//! and Mode B feeds to the annotation pass as engine-grounded truth.
//!
//! Layering: [`classify`] and [`explain`] are pure and unit-tested like
//! [`crate::features`]; [`service`] is the thin engine-driven shell; [`routes`]
//! is the HTTP transport.

pub mod classify;
pub mod explain;
pub mod routes;
pub mod service;

pub use classify::{Classification, ReviewSummary, SideSummary};
pub use explain::MoveFact;
pub use service::{review_game, GameReview, MoveReview, ReviewError};
