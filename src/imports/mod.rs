//! Transport-agnostic import service: trigger a provider sync (Lichess /
//! Chess.com) or ingest an uploaded PGN into a target database. Thin
//! orchestration over the [`collectors`](crate::collectors) and the shared
//! [`ingest`](crate::ingest) pipeline, so the HTTP routes (and a future MCP tool)
//! are thin callers — the write guard and provider dispatch live here.
//!
//! Ownership follows ADR 0007 / 0011: a sync/upload may only target a database
//! the caller can write — their own, or (as admin) a global one.

use sea_orm::{DatabaseConnection, DbErr, EntityTrait};

use crate::collectors::{chesscom::ChessCom, lichess::Lichess, SyncCursor};
use crate::db::entities::databases;
use crate::ingest::ingest_pgn_all;
use crate::server::identity::{assert_can_write, CurrentUser};

mod cursor;
pub mod routes;

/// Why an import failed. Transport-agnostic — the HTTP / MCP layer maps each
/// variant onto its own status / error envelope.
#[derive(Debug, thiserror::Error)]
pub enum ImportError {
    /// No database with that id exists.
    #[error("database not found")]
    NotFound,
    /// Authenticated but not permitted to write the target database.
    #[error("not permitted")]
    Forbidden,
    /// A required field was blank or invalid (empty PGN/username, unknown source).
    #[error("{0}")]
    InvalidInput(String),
    /// A provider sync failed (network, bad username/token). Carries a curated,
    /// client-safe message — never a raw `DbErr`, reqwest, or anyhow chain; those
    /// are logged server-side instead.
    #[error("{0}")]
    Failed(String),
    /// Underlying database error (never surfaced verbatim to clients).
    #[error(transparent)]
    Db(#[from] DbErr),
}

/// Which provider a sync pulls from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImportSource {
    Lichess,
    ChessCom,
}

impl ImportSource {
    /// Parse the wire tag (`"lichess"` / `"chesscom"`), case-insensitively.
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "lichess" => Some(Self::Lichess),
            "chesscom" | "chess.com" => Some(Self::ChessCom),
            _ => None,
        }
    }

    /// Canonical tag stored in `sync_cursors.source` (matches `GameSource::kind`).
    fn as_str(self) -> &'static str {
        match self {
            Self::Lichess => "lichess",
            Self::ChessCom => "chesscom",
        }
    }
}

/// Outcome reported to clients. A multi-game PGN upload is skip-and-continue, so
/// a partial success still returns this summary (HTTP 200) rather than aborting:
/// `imported` games stored (their ids in `game_ids`, in PGN order, so a client
/// can chain the new game into further calls), `duplicates` dropped as already
/// present, `skipped` games dropped as bad, with one client-safe `errors` entry
/// per skipped game. A provider sync reports real `imported`/`duplicates`
/// counts and `synced_at` (issue #197); `game_ids` stays empty (cursor-boundary
/// dedup means "imported" is not a stable list of new ids) and `synced_at` is
/// `None` for a PGN upload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportSummary {
    pub imported: usize,
    pub skipped: usize,
    pub duplicates: usize,
    pub game_ids: Vec<i32>,
    pub errors: Vec<String>,
    pub synced_at: Option<String>,
}

/// Import orchestration over the `databases` table + collectors. Holds a
/// connection handle (cheap to clone — SeaORM wraps an `Arc`'d pool).
#[derive(Clone)]
pub struct ImportService {
    db: DatabaseConnection,
    /// Test-only override so `sync`'s Chess.com dispatch can be pointed at a
    /// local mock server instead of the real API — makes the failure/full-sync
    /// cursor-persistence paths (#197) exercisable without network access.
    #[cfg(test)]
    chesscom_base_url: Option<String>,
}

impl ImportService {
    pub fn new(db: DatabaseConnection) -> Self {
        Self {
            db,
            #[cfg(test)]
            chesscom_base_url: None,
        }
    }

    #[cfg(test)]
    fn with_chesscom_base_url(mut self, base_url: impl Into<String>) -> Self {
        self.chesscom_base_url = Some(base_url.into());
        self
    }

    /// Ingest a (possibly multi-game) PGN into a database the caller may write.
    pub async fn import_pgn(
        &self,
        user: &CurrentUser,
        database_id: i32,
        pgn: &str,
    ) -> Result<ImportSummary, ImportError> {
        self.load_writable(user, database_id).await?;
        if pgn.trim().is_empty() {
            return Err(ImportError::InvalidInput("PGN is empty".into()));
        }
        // Skip-and-continue: a bad game is recorded, not fatal. Only a genuine
        // storage failure (`DbErr`) aborts, mapping to a generic 500.
        let report = ingest_pgn_all(&self.db, database_id, pgn).await?;
        Ok(ImportSummary {
            imported: report.imported.len(),
            skipped: report.errors.len(),
            duplicates: report.duplicates,
            game_ids: report.imported.iter().map(|g| g.game_id).collect(),
            errors: report
                .errors
                .iter()
                .map(|e| format!("game {}: {}", e.index, e.message))
                .collect(),
            synced_at: None,
        })
    }

    /// Trigger a provider sync into a database the caller may write. A blank
    /// `token` is treated as absent. The sync resumes from the cursor persisted
    /// per `(database, source, username)`; `full` ignores that stored cursor and
    /// starts from scratch (issue #197). The advanced cursor — and real
    /// imported/duplicate counts — are saved whether the run succeeds *or*
    /// fails partway through, so a history too large to finish in one request
    /// makes progress across retries instead of restarting from zero every
    /// time (#197); ingest dedup keeps the boundary month/second the cursor
    /// re-fetches from doubling games (#95).
    pub async fn sync(
        &self,
        user: &CurrentUser,
        database_id: i32,
        source: ImportSource,
        username: &str,
        token: Option<&str>,
        full: bool,
    ) -> Result<ImportSummary, ImportError> {
        self.load_writable(user, database_id).await?;
        let username = username.trim();
        if username.is_empty() {
            return Err(ImportError::InvalidInput("username is required".into()));
        }
        let token = token.map(str::trim).filter(|t| !t.is_empty());

        let cursor = if full {
            SyncCursor::default()
        } else {
            cursor::load(&self.db, database_id, source.as_str(), username).await?
        };

        let result = match source {
            ImportSource::Lichess => {
                let mut src = Lichess::new(username);
                if let Some(token) = token {
                    src = src.with_token(token);
                }
                src.sync(&self.db, database_id, cursor).await
            }
            ImportSource::ChessCom => {
                #[allow(unused_mut)]
                let mut src = ChessCom::new(username);
                #[cfg(test)]
                if let Some(base) = &self.chesscom_base_url {
                    src = src.with_base_url(base.clone());
                }
                src.sync(&self.db, database_id, cursor).await
            }
        };

        let (outcome_cursor, imported, duplicates, failed) = match result {
            Ok(outcome) => (outcome.cursor, outcome.imported, outcome.duplicates, false),
            Err(failure) => {
                // The provider/anyhow chain can carry reqwest URLs or wrapped
                // SQL — log it server-side, hand the client a generic message.
                tracing::warn!(error = ?failure.error, source = ?source, username, "provider sync failed");
                (failure.cursor, failure.imported, failure.duplicates, true)
            }
        };

        // Persist whatever progress was made even on failure (#197) — a
        // mid-run 429/dropped-stream must not discard an already-advanced
        // cursor, or a large history never finishes syncing.
        let synced_at = cursor::save(
            &self.db,
            database_id,
            source.as_str(),
            username,
            &outcome_cursor,
            imported,
            duplicates,
        )
        .await?;

        if failed {
            return Err(ImportError::Failed(
                "sync failed — check the username and token, then try again".into(),
            ));
        }

        Ok(ImportSummary {
            imported,
            skipped: 0,
            duplicates,
            game_ids: Vec::new(),
            errors: Vec::new(),
            synced_at: Some(synced_at.and_utc().to_rfc3339()),
        })
    }

    /// Load a database by id and enforce the write guard: the caller must own it,
    /// or be admin for a global one. `NotFound` hides ids that don't exist.
    async fn load_writable(
        &self,
        user: &CurrentUser,
        id: i32,
    ) -> Result<databases::Model, ImportError> {
        let model = databases::Entity::find_by_id(id)
            .one(&self.db)
            .await?
            .ok_or(ImportError::NotFound)?;
        assert_can_write(model.owner_id.as_deref(), user).map_err(|_| ImportError::Forbidden)?;
        Ok(model)
    }
}

#[cfg(test)]
#[path = "mod_tests.rs"]
mod tests;
