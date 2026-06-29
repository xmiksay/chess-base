//! chess-base — self-hosted ChessBase-like application.
//!
//! Layered for testability:
//! - [`position`] and [`pgn_tree`] are **pure** (no I/O) and fully unit-testable.
//! - [`db`], [`collectors`], [`engine`] and [`server`] are thin, dependency-injected
//!   adapters around that core.

pub mod ai;
pub mod auth;
pub mod collectors;
pub mod databases;
pub mod db;
pub mod engine;
pub mod games;
pub mod ingest;
pub mod openings;
pub mod pgn_tree;
pub mod plans;
pub mod position;
pub mod search;
pub mod server;
pub mod settings;
pub mod studies;
pub mod study_gen;

pub use ingest::{ingest_pgn, Ingested};
pub use openings::{classify_mainline, eco_of_position, opening_of_zobrist, Opening};
pub use pgn_tree::pgn::{from_pgn, to_pgn, PgnError};
pub use pgn_tree::{MoveTree, Node};
pub use plans::{plan_from_pv, Plan, Trajectory};
pub use position::{
    legal_sans, position_from_fen, zobrist_of_fen, CastlingMode, PositionError, STARTPOS_FEN,
};
