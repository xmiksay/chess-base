//! chess-base — self-hosted ChessBase-like application.
//!
//! Layered for testability:
//! - [`position`] and [`pgn_tree`] are **pure** (no I/O) and fully unit-testable.
//! - [`db`], [`collectors`], [`engine`] and [`server`] are thin, dependency-injected
//!   adapters around that core.

pub mod collectors;
pub mod db;
pub mod engine;
pub mod pgn_tree;
pub mod position;
pub mod server;

pub use pgn_tree::{MoveTree, Node};
pub use position::{legal_sans, position_from_fen, zobrist_of_fen, PositionError, STARTPOS_FEN};
