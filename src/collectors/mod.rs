//! Game collection: a common [`GameSource`] abstraction plus per-provider
//! adapters. The scaffold provides the types, endpoint builders and sync-cursor
//! model; the actual networked fetch/import is implemented by the Epic 2 issues.

pub mod chesscom;
pub mod lichess;

use serde::{Deserialize, Serialize};

/// Incremental sync position, persisted per source so re-syncs are cheap.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncCursor {
    /// Last fully-synced month, `"YYYY/MM"` (archive-based sources, e.g. Chess.com).
    pub last_month: Option<String>,
    /// Epoch-ms of the most recently synced game (stream-based sources, e.g. Lichess).
    pub last_game_ms: Option<i64>,
}

/// A provider of chess games. Implementors expose where games are pulled from;
/// the sync engine drives the actual download into a target database.
pub trait GameSource {
    /// Tag stored on the `databases.kind` column for collections from this source.
    fn kind(&self) -> &'static str;
}
