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
use crate::server::identity::{assert_admin, CurrentUser};

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
    /// The collector sync or PGN ingest failed (network, malformed PGN, illegal
    /// move). The message comes from the collector/ingest layer — never a raw
    /// `DbErr` — so it is safe to surface to clients.
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
}

/// Outcome reported to clients: how many games were ingested this run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ImportSummary {
    pub imported: usize,
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
        let ingested = ingest_pgn_all(&self.db, database_id, pgn)
            .await
            .map_err(|e| ImportError::Failed(e.to_string()))?;
        Ok(ImportSummary {
            imported: ingested.len(),
        })
    }

    /// Trigger a provider sync into a database the caller may write. A blank
    /// `token` is treated as absent. Syncs start from a fresh cursor (full sync);
    /// persisting the returned cursor for incremental re-syncs is a later epic.
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

        let outcome = match source {
            ImportSource::Lichess => {
                let mut src = Lichess::new(username);
                if let Some(token) = token {
                    src = src.with_token(token);
                }
                src.sync(&self.db, database_id, SyncCursor::default()).await
            }
            ImportSource::ChessCom => {
                ChessCom::new(username)
                    .sync(&self.db, database_id, SyncCursor::default())
                    .await
            }
        }
        .map_err(|e| ImportError::Failed(e.to_string()))?;

        Ok(ImportSummary {
            imported: outcome.imported,
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
        assert_can_write(&model, user)?;
        Ok(model)
    }
}

/// Write guard (ADR 0007 / 0011): a database is writable only by its owner; a
/// global database (`owner_id IS NULL`) requires admin.
fn assert_can_write(model: &databases::Model, user: &CurrentUser) -> Result<(), ImportError> {
    match &model.owner_id {
        None => assert_admin(user).map_err(|_| ImportError::Forbidden),
        Some(owner) if *owner == user.id => Ok(()),
        Some(_) => Err(ImportError::Forbidden),
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
        assert_eq!(games::Entity::find().all(&svc.db).await.unwrap().len(), 2);
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
