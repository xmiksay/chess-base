//! m0009 — sync-cursor lifecycle fixes (issue #197, ADR-0020 update):
//!
//! - `username` on `sync_cursors`, folded into the uniqueness key alongside
//!   `(database_id, source)`, so syncing a different provider username into the
//!   same collection gets its own cursor instead of inheriting (and silently
//!   corrupting) the wrong one. Existing rows keep `username = NULL` and simply
//!   stop matching once a sync runs (the next sync for that database/source
//!   starts a one-off full re-sync — safe, since ingest dedups by `source_ref`).
//! - `last_synced_at` / `last_imported` / `last_duplicates` so the UI can show
//!   "last synced …, N new / M already present" instead of nothing.
//!
//! Schema-builder only, so the same migration runs on SQLite and Postgres.

use sea_orm_migration::prelude::*;

pub struct Migration;

impl MigrationName for Migration {
    fn name(&self) -> &str {
        "m0009_sync_cursor_lifecycle"
    }
}

#[derive(DeriveIden)]
enum SyncCursors {
    Table,
    DatabaseId,
    Source,
    Username,
    LastSyncedAt,
    LastImported,
    LastDuplicates,
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(SyncCursors::Table)
                    .add_column(ColumnDef::new(SyncCursors::Username).string().null())
                    .to_owned(),
            )
            .await?;
        manager
            .alter_table(
                Table::alter()
                    .table(SyncCursors::Table)
                    .add_column(ColumnDef::new(SyncCursors::LastSyncedAt).timestamp().null())
                    .to_owned(),
            )
            .await?;
        manager
            .alter_table(
                Table::alter()
                    .table(SyncCursors::Table)
                    .add_column(ColumnDef::new(SyncCursors::LastImported).integer().null())
                    .to_owned(),
            )
            .await?;
        manager
            .alter_table(
                Table::alter()
                    .table(SyncCursors::Table)
                    .add_column(ColumnDef::new(SyncCursors::LastDuplicates).integer().null())
                    .to_owned(),
            )
            .await?;

        manager
            .drop_index(
                Index::drop()
                    .name("idx_sync_cursors_database_source")
                    .table(SyncCursors::Table)
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .name("idx_sync_cursors_database_source_username")
                    .table(SyncCursors::Table)
                    .col(SyncCursors::DatabaseId)
                    .col(SyncCursors::Source)
                    .col(SyncCursors::Username)
                    .unique()
                    .to_owned(),
            )
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_index(
                Index::drop()
                    .name("idx_sync_cursors_database_source_username")
                    .table(SyncCursors::Table)
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .name("idx_sync_cursors_database_source")
                    .table(SyncCursors::Table)
                    .col(SyncCursors::DatabaseId)
                    .col(SyncCursors::Source)
                    .unique()
                    .to_owned(),
            )
            .await?;

        for col in [
            SyncCursors::LastDuplicates,
            SyncCursors::LastImported,
            SyncCursors::LastSyncedAt,
            SyncCursors::Username,
        ] {
            manager
                .alter_table(
                    Table::alter()
                        .table(SyncCursors::Table)
                        .drop_column(col)
                        .to_owned(),
                )
                .await?;
        }
        Ok(())
    }
}
