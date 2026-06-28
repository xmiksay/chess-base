//! chess-base — self-hosted ChessBase-like application.
//!
//! Layered for testability:
//! - [`position`] and [`pgn_tree`] are **pure** (no I/O) and fully unit-testable.
//! - [`db`], [`collectors`], [`engine`] and [`server`] are thin, dependency-injected
//!   adapters around that core.

pub mod ai;
pub mod collectors;
pub mod db;
pub mod engine;
pub mod openings;
pub mod pgn_tree;
pub mod position;
pub mod server;
pub mod studies;

pub use openings::{classify_mainline, eco_of_position, opening_of_zobrist, Opening};
pub use pgn_tree::pgn::{from_pgn, to_pgn, PgnError};
pub use pgn_tree::{MoveTree, Node};
pub use position::{
    legal_sans, position_from_fen, zobrist_of_fen, CastlingMode, PositionError, STARTPOS_FEN,
};
