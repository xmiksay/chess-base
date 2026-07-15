//! Transport-agnostic import service: trigger a provider sync (Lichess /
//! Chess.com) or ingest an uploaded PGN into a target database. Thin
//! orchestration over the [`collectors`](crate::collectors) and the shared
//! [`ingest`](crate::ingest) pipeline, so the HTTP routes (and a future MCP tool)
//! are thin callers — the write guard and provider dispatch live here.
//!
//! Ownership follows ADR 0007 / 0011: a sync/upload may only target a database
//! the caller can write — their own, or (as admin) a global one.

use sea_orm::{DatabaseConnection, DbErr, EntityTrait};

use crate::collectors::{chesscom::ChessCom, lichess::Lichess};
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
/// per skipped game. A provider sync reports counts only (`game_ids` empty).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportSummary {
    pub imported: usize,
    pub skipped: usize,
    pub duplicates: usize,
    pub game_ids: Vec<i32>,
    pub errors: Vec<String>,
}

/// Import orchestration over the `databases` table + collectors. Holds a
/// connection handle (cheap to clone — SeaORM wraps an `Arc`'d pool).
#[derive(Clone)]
pub struct ImportService {
    db: DatabaseConnection,
}

impl ImportService {
    pub fn new(db: DatabaseConnection) -> Self {
        Self { db }
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
        })
    }

    /// Trigger a provider sync into a database the caller may write. A blank
    /// `token` is treated as absent. The sync resumes from the cursor persisted
    /// per `(database, source)` and saves the advanced cursor afterwards, so a
    /// re-sync only fetches new games (issue #95); ingest dedup keeps the boundary
    /// month/second the cursor re-fetches from doubling games.
    pub async fn sync(
        &self,
        user: &CurrentUser,
        database_id: i32,
        source: ImportSource,
        username: &str,
        token: Option<&str>,
    ) -> Result<ImportSummary, ImportError> {
        self.load_writable(user, database_id).await?;
        let username = username.trim();
        if username.is_empty() {
            return Err(ImportError::InvalidInput("username is required".into()));
        }
        let token = token.map(str::trim).filter(|t| !t.is_empty());

        let cursor = cursor::load(&self.db, database_id, source.as_str()).await?;

        let outcome = match source {
            ImportSource::Lichess => {
                let mut src = Lichess::new(username);
                if let Some(token) = token {
                    src = src.with_token(token);
                }
                src.sync(&self.db, database_id, cursor).await
            }
            ImportSource::ChessCom => {
                ChessCom::new(username)
                    .sync(&self.db, database_id, cursor)
                    .await
            }
        }
        .map_err(|e| {
            // The provider/anyhow chain can carry reqwest URLs or wrapped SQL —
            // log it server-side, hand the client a generic, actionable message.
            tracing::warn!(error = ?e, source = ?source, username, "provider sync failed");
            ImportError::Failed("sync failed — check the username and token, then try again".into())
        })?;

        cursor::save(&self.db, database_id, source.as_str(), &outcome.cursor).await?;

        // Sync is bulk-scale and its cursor-boundary dedup is intended (#95), so
        // it reports the imported count only.
        Ok(ImportSummary {
            imported: outcome.imported,
            skipped: 0,
            duplicates: 0,
            game_ids: Vec::new(),
            errors: Vec::new(),
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
mod tests {
    use super::*;
    use crate::db::entities::games;
    use crate::db::{connect, DbConfig};
    use sea_orm::{ActiveModelTrait, Set};

    const TWO_GAMES: &str = "[Event \"Game 1\"]\n[White \"Spassky\"]\n[Black \"Fischer\"]\n[Result \"1-0\"]\n\n1. e4 e5 2. Bc4 Nc6 3. Qh5 Nf6 4. Qxf7# 1-0\n\n[Event \"Game 2\"]\n[White \"Carlsen\"]\n[Black \"Caruana\"]\n[Result \"1/2-1/2\"]\n\n1. d4 d5 2. c4 e6 1/2-1/2\n";

    fn user(id: &str) -> CurrentUser {
        CurrentUser {
            id: id.to_string(),
            is_admin: false,
        }
    }

    async fn service_with_db(owner: Option<&str>) -> (ImportService, i32) {
        let conn = connect(&DbConfig::in_memory()).await.unwrap();
        let db = databases::ActiveModel {
            owner_id: Set(owner.map(str::to_string)),
            name: Set("Games".to_string()),
            kind: Set("own".to_string()),
            ..Default::default()
        }
        .insert(&conn)
        .await
        .unwrap();
        (ImportService::new(conn.clone()), db.id)
    }

    #[test]
    fn parses_known_sources_case_insensitively() {
        assert_eq!(ImportSource::parse("Lichess"), Some(ImportSource::Lichess));
        assert_eq!(
            ImportSource::parse("chesscom"),
            Some(ImportSource::ChessCom)
        );
        assert_eq!(
            ImportSource::parse("chess.com"),
            Some(ImportSource::ChessCom)
        );
        assert_eq!(ImportSource::parse("fics"), None);
    }

    #[tokio::test]
    async fn import_pgn_ingests_every_game_into_an_owned_database() {
        let (svc, id) = service_with_db(Some("alice")).await;
        let summary = svc.import_pgn(&user("alice"), id, TWO_GAMES).await.unwrap();
        assert_eq!(summary.imported, 2);
        assert_eq!(summary.skipped, 0);
        assert_eq!(summary.duplicates, 0);
        assert!(summary.errors.is_empty());

        // The new games' ids come back (in PGN order) so a client can chain them.
        let stored = games::Entity::find().all(&svc.db).await.unwrap();
        assert_eq!(
            summary.game_ids,
            stored.iter().map(|g| g.id).collect::<Vec<_>>()
        );
    }

    #[tokio::test]
    async fn import_pgn_reports_duplicates_on_reupload() {
        let (svc, id) = service_with_db(Some("alice")).await;
        svc.import_pgn(&user("alice"), id, TWO_GAMES).await.unwrap();

        let summary = svc.import_pgn(&user("alice"), id, TWO_GAMES).await.unwrap();
        assert_eq!(summary.imported, 0);
        assert_eq!(summary.duplicates, 2);
        assert!(summary.game_ids.is_empty());
        assert!(summary.errors.is_empty());
        assert_eq!(games::Entity::find().all(&svc.db).await.unwrap().len(), 2);
    }

    // One legal game then an illegal one (Black answers 1. e4 with another e4).
    const ONE_GOOD_ONE_BAD: &str = "[Event \"Good\"]\n[White \"A\"]\n[Black \"B\"]\n[Result \"1-0\"]\n\n1. e4 e5 2. Bc4 Nc6 3. Qh5 Nf6 4. Qxf7# 1-0\n\n[Event \"Bad\"]\n[White \"C\"]\n[Black \"D\"]\n[Result \"*\"]\n\n1. e4 e4 *\n";

    #[tokio::test]
    async fn import_pgn_skips_a_bad_game_and_reports_it() {
        let (svc, id) = service_with_db(Some("alice")).await;
        let summary = svc
            .import_pgn(&user("alice"), id, ONE_GOOD_ONE_BAD)
            .await
            .unwrap();
        // Partial success is not an error: the good game lands, the bad one is
        // reported with a safe, indexed message (no leaked SQL / provider chain).
        assert_eq!(summary.imported, 1);
        assert_eq!(summary.skipped, 1);
        assert_eq!(summary.errors.len(), 1);
        assert!(summary.errors[0].starts_with("game 2:"));
        assert_eq!(games::Entity::find().all(&svc.db).await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn import_pgn_rejects_empty_input() {
        let (svc, id) = service_with_db(Some("alice")).await;
        assert!(matches!(
            svc.import_pgn(&user("alice"), id, "  \n ")
                .await
                .unwrap_err(),
            ImportError::InvalidInput(_)
        ));
    }

    #[tokio::test]
    async fn import_pgn_forbids_writing_another_users_database() {
        let (svc, id) = service_with_db(Some("alice")).await;
        assert!(matches!(
            svc.import_pgn(&user("bob"), id, TWO_GAMES)
                .await
                .unwrap_err(),
            ImportError::Forbidden
        ));
    }

    #[tokio::test]
    async fn import_pgn_reports_a_missing_database() {
        let (svc, _) = service_with_db(Some("alice")).await;
        assert!(matches!(
            svc.import_pgn(&user("alice"), 9999, TWO_GAMES)
                .await
                .unwrap_err(),
            ImportError::NotFound
        ));
    }

    #[tokio::test]
    async fn global_database_requires_admin_to_import() {
        let (svc, id) = service_with_db(None).await; // global (owner_id NULL)
                                                     // A non-admin is forbidden; the implicit admin succeeds.
        assert!(matches!(
            svc.import_pgn(&user("bob"), id, TWO_GAMES)
                .await
                .unwrap_err(),
            ImportError::Forbidden
        ));
        let summary = svc
            .import_pgn(&CurrentUser::local_admin(), id, TWO_GAMES)
            .await
            .unwrap();
        assert_eq!(summary.imported, 2);
        assert_eq!(summary.skipped, 0);
    }

    #[tokio::test]
    async fn sync_requires_a_username_after_the_write_guard() {
        let (svc, id) = service_with_db(Some("alice")).await;
        assert!(matches!(
            svc.sync(&user("alice"), id, ImportSource::Lichess, "  ", None)
                .await
                .unwrap_err(),
            ImportError::InvalidInput(_)
        ));
    }
}
