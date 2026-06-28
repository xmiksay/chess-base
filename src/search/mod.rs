//! Position search (ADR-0003): "find games reaching this position" plus the
//! opening tree of aggregated move statistics, keyed on the Zobrist
//! `position_index`. The [`position`] submodule holds the transport-agnostic
//! [`PositionSearchService`]; [`routes`] is the thin NDJSON-streaming HTTP layer.

pub mod position;
pub mod routes;

pub use position::{GameHit, MoveStat, PositionSearchService, SearchError};
