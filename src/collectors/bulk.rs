//! Bulk master-database PGN importer (issue #4).
//!
//! Free master sources (Lumbras Giga Base, Ajedrez Data / TWIC) ship plain PGN
//! or `.pgn.zst` with millions of games. This importer streams such a file in
//! bounded memory and funnels every game through the shared [`ingest`] pipeline,
//! committing in batched transactions for throughput.
//!
//! - **Streaming.** The input is read in fixed-size chunks; complete games are
//!   drained from a buffer that only ever holds one in-flight game plus a chunk,
//!   so a multi-GB file imports without loading it whole. A `.zst` path is
//!   transparently decompressed.
//! - **Dedup + restart.** Master games carry no provider permalink, so they are
//!   deduplicated by a content hash ([`ParsedGame::content_hash`]). The hash is
//!   stored as the game's `source_ref`, whose unique `(database_id, source_ref)`
//!   index makes a re-run skip everything already imported — the import is
//!   restartable. Duplicates within the same file are caught in-memory.
//! - **Bulk tuning.** On SQLite the connection gets bulk-insert PRAGMAs (WAL,
//!   `synchronous = NORMAL`, …) before the run.
//!
//! File reads are blocking; this is an admin/CLI path, not a server hot path.

use std::collections::HashSet;
use std::io::{BufReader, Read};
use std::path::Path;

use anyhow::{Context, Result};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, DatabaseBackend, DatabaseConnection,
    EntityTrait, QueryFilter, Set, Statement, TransactionTrait,
};

use crate::db::entities::databases;
use crate::ingest::{
    event_offsets, game_exists, load_index_depth, parse_pgn, prepare_game, split_games,
    store_prepared, PreparedGame,
};

/// How many games one transaction commits before starting the next.
const DEFAULT_BATCH_SIZE: usize = 1_000;

/// Bytes read from the input per `read` call. Bounds peak memory together with
/// the trailing in-flight game and the current batch.
const CHUNK_SIZE: usize = 64 * 1024;

/// Tally of a bulk import run.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct BulkReport {
    /// Games newly stored and indexed.
    pub imported: usize,
    /// Games skipped because an identical game was already present (a prior run
    /// or earlier in this file).
    pub duplicates: usize,
    /// Games skipped as unparseable or illegal (skip-and-continue).
    pub errors: usize,
}

/// A validated game waiting to be committed in the current batch.
struct Pending {
    prepared: PreparedGame,
    pgn: String,
    source_ref: String,
}

/// Streaming bulk importer. Cheap to construct; holds only its batch size.
pub struct BulkImporter {
    batch_size: usize,
}

impl Default for BulkImporter {
    fn default() -> Self {
        Self {
            batch_size: DEFAULT_BATCH_SIZE,
        }
    }
}

impl BulkImporter {
    pub fn new() -> Self {
        Self::default()
    }

    /// Override the per-transaction batch size (clamped to at least 1).
    pub fn with_batch_size(mut self, batch_size: usize) -> Self {
        self.batch_size = batch_size.max(1);
        self
    }

    /// Import every game from `path` into `database_id`. A `.zst` file is
    /// decompressed on the fly; any other extension is read as plain PGN.
    pub async fn import_path(
        &self,
        db: &DatabaseConnection,
        database_id: i32,
        path: &Path,
    ) -> Result<BulkReport> {
        let file = std::fs::File::open(path)
            .with_context(|| format!("opening PGN file {}", path.display()))?;
        let reader = BufReader::new(file);
        let is_zst = path
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("zst"));
        if is_zst {
            let decoder = zstd::stream::read::Decoder::new(reader)
                .with_context(|| format!("opening zstd stream {}", path.display()))?;
            self.import_reader(db, database_id, decoder).await
        } else {
            self.import_reader(db, database_id, reader).await
        }
    }

    /// Import every game read from `src` (already-decompressed PGN bytes) into
    /// `database_id`. The transport-agnostic core, exercised directly by tests.
    pub async fn import_reader<R: Read>(
        &self,
        db: &DatabaseConnection,
        database_id: i32,
        mut src: R,
    ) -> Result<BulkReport> {
        apply_bulk_pragmas(db).await?;
        let index_depth = load_index_depth(db, database_id).await?;

        let mut report = BulkReport::default();
        let mut seen: HashSet<String> = HashSet::new();
        let mut batch: Vec<Pending> = Vec::new();
        let mut buf: Vec<u8> = Vec::new();
        let mut chunk = [0u8; CHUNK_SIZE];

        loop {
            let n = src.read(&mut chunk).context("reading PGN stream")?;
            if n == 0 {
                break;
            }
            buf.extend_from_slice(&chunk[..n]);

            // Drain every provably-complete game: a new `[Event ` after the first
            // means everything before it is whole. The trailing (possibly partial)
            // game stays buffered until more bytes — or EOF — arrive.
            let starts = event_offsets(&buf);
            if starts.len() >= 2 {
                let split = starts[starts.len() - 1];
                let tail = buf.split_off(split);
                let head = std::mem::replace(&mut buf, tail);
                self.process_blob(
                    db,
                    database_id,
                    index_depth,
                    &String::from_utf8_lossy(&head),
                    &mut seen,
                    &mut batch,
                    &mut report,
                )
                .await?;
            }
        }

        // Flush the trailing game, then commit whatever the last batch holds.
        self.process_blob(
            db,
            database_id,
            index_depth,
            &String::from_utf8_lossy(&buf),
            &mut seen,
            &mut batch,
            &mut report,
        )
        .await?;
        flush(db, database_id, index_depth, &mut batch, &mut report).await?;

        tracing::info!(
            imported = report.imported,
            duplicates = report.duplicates,
            errors = report.errors,
            "bulk import finished"
        );
        Ok(report)
    }

    /// Process every game in one complete blob: parse, dedup, validate and queue
    /// it, flushing the batch whenever it fills. Bad games are counted and
    /// skipped; only a storage failure aborts.
    #[allow(clippy::too_many_arguments)]
    async fn process_blob(
        &self,
        db: &DatabaseConnection,
        database_id: i32,
        index_depth: Option<i32>,
        blob: &str,
        seen: &mut HashSet<String>,
        batch: &mut Vec<Pending>,
        report: &mut BulkReport,
    ) -> Result<()> {
        for game_pgn in split_games(blob) {
            let parsed = match parse_pgn(&game_pgn) {
                Ok(parsed) => parsed,
                Err(_) => {
                    report.errors += 1;
                    continue;
                }
            };
            let source_ref = parsed.content_hash();

            // In-file dedup (cheap) then cross-run dedup against committed games.
            if !seen.insert(source_ref.clone()) {
                report.duplicates += 1;
                continue;
            }
            if game_exists(db, database_id, &source_ref).await? {
                report.duplicates += 1;
                continue;
            }

            let prepared = match prepare_game(parsed) {
                Ok(prepared) => prepared,
                Err(_) => {
                    report.errors += 1;
                    continue;
                }
            };
            batch.push(Pending {
                prepared,
                pgn: game_pgn,
                source_ref,
            });
            if batch.len() >= self.batch_size {
                flush(db, database_id, index_depth, batch, report).await?;
            }
        }
        Ok(())
    }
}

/// Commit the queued batch in one transaction, then clear it. Empty is a no-op.
async fn flush(
    db: &DatabaseConnection,
    database_id: i32,
    index_depth: Option<i32>,
    batch: &mut Vec<Pending>,
    report: &mut BulkReport,
) -> Result<()> {
    if batch.is_empty() {
        return Ok(());
    }
    let txn = db.begin().await.context("opening bulk batch transaction")?;
    for pending in batch.iter() {
        store_prepared(
            &txn,
            database_id,
            &pending.prepared,
            &pending.pgn,
            Some(pending.source_ref.clone()),
            index_depth,
        )
        .await
        .context("storing bulk game")?;
        report.imported += 1;
    }
    txn.commit().await.context("committing bulk batch")?;
    batch.clear();

    tracing::info!(
        imported = report.imported,
        duplicates = report.duplicates,
        "bulk import progress"
    );
    Ok(())
}

/// Find the global `master` database named `name`, creating it if absent. Lets a
/// CLI bulk import target a stable, admin-owned collection idempotently.
pub async fn find_or_create_master(db: &DatabaseConnection, name: &str) -> Result<i32> {
    if let Some(existing) = databases::Entity::find()
        .filter(databases::Column::OwnerId.is_null())
        .filter(databases::Column::Name.eq(name))
        .one(db)
        .await
        .context("looking up master database")?
    {
        return Ok(existing.id);
    }
    let model = databases::ActiveModel {
        owner_id: Set(None),
        name: Set(name.to_string()),
        kind: Set("master".to_string()),
        index_depth: Set(databases::default_index_depth("master")),
        ..Default::default()
    }
    .insert(db)
    .await
    .context("creating master database")?;
    Ok(model.id)
}

/// Apply SQLite bulk-insert PRAGMAs to the connection. A no-op on Postgres. WAL
/// persists in the file header; the rest are per-connection best-effort tuning,
/// which is fine for the sequential bulk path.
async fn apply_bulk_pragmas(db: &DatabaseConnection) -> Result<()> {
    if db.get_database_backend() != DatabaseBackend::Sqlite {
        return Ok(());
    }
    for pragma in [
        "PRAGMA journal_mode = WAL;",
        "PRAGMA synchronous = NORMAL;",
        "PRAGMA temp_store = MEMORY;",
        // ~64 MiB page cache (negative = kibibytes).
        "PRAGMA cache_size = -65536;",
    ] {
        db.execute(Statement::from_string(DatabaseBackend::Sqlite, pragma))
            .await
            .with_context(|| format!("applying {pragma}"))?;
    }
    Ok(())
}

#[cfg(test)]
#[path = "bulk_tests.rs"]
mod tests;
