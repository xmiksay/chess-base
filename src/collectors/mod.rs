//! Game collection: a common [`GameSource`] abstraction plus per-provider
//! adapters. The scaffold provides the types, endpoint builders and sync-cursor
//! model; the actual networked fetch/import is implemented by the Epic 2 issues.

pub mod chesscom;
pub mod lichess;

use std::time::Duration;

use serde::{Deserialize, Serialize};

/// Back-off delay for an HTTP 429: at least `min_backoff`, or the server's
/// `Retry-After` (seconds) when it asks for longer. Shared by every collector;
/// each passes its own provider-specific floor.
pub(crate) fn backoff_delay(retry_after: Option<u64>, min_backoff: Duration) -> Duration {
    match retry_after {
        Some(secs) => min_backoff.max(Duration::from_secs(secs)),
        None => min_backoff,
    }
}

/// Parse the `Retry-After` header (delay in seconds) if present and numeric.
pub(crate) fn retry_after_secs(resp: &reqwest::Response) -> Option<u64> {
    resp.headers()
        .get(reqwest::header::RETRY_AFTER)?
        .to_str()
        .ok()?
        .trim()
        .parse()
        .ok()
}

/// Incremental sync position, persisted per source so re-syncs are cheap.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncCursor {
    /// Last fully-synced month, `"YYYY/MM"` (archive-based sources, e.g. Chess.com).
    pub last_month: Option<String>,
    /// Epoch-ms of the most recently synced game (stream-based sources, e.g. Lichess).
    pub last_game_ms: Option<i64>,
}

/// Result of a sync run: the advanced cursor to persist and how many games were
/// ingested this run. Shared by every collector ([`lichess`] / [`chesscom`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncOutcome {
    pub cursor: SyncCursor,
    pub imported: usize,
}

/// A provider of chess games. Implementors expose where games are pulled from;
/// the sync engine drives the actual download into a target database.
pub trait GameSource {
    /// Tag stored on the `databases.kind` column for collections from this source.
    fn kind(&self) -> &'static str;
}
