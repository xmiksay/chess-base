//! Game search. Position search (ADR-0003) — "find games reaching this position"
//! plus the opening tree — keyed on the Zobrist `position_index`; header search
//! (issue #6) — query games by player/event/ECO/date/result with keyset
//! pagination. The submodules hold the transport-agnostic services; [`routes`]
//! is the thin HTTP layer over both. The [`report`] submodule layers the
//! pre-chewed DB query surface (issue #28) on top of position search: ECO,
//! per-move frequency/score, transpositions and reference games.

pub mod headers;
pub mod position;
pub mod report;
pub mod routes;

pub use headers::{HeaderPage, HeaderQuery, HeaderSearchError, HeaderSearchService};
pub use position::{Color, GameHit, MoveStat, PositionFilter, PositionSearchService, SearchError};
pub use report::{EcoInfo, MoveReport, PositionReport, PositionReportService, Transposition};
