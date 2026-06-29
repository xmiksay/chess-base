//! m0005 — incremental-sync support (issue #95): a stable per-game `source_ref`
//! on `games` (unique per database) so a re-sync dedups instead of doubling, and
//! a `sync_cursors` table persisting the per-(database, source) resume position.
//! Schema-builder only, so it runs on SQLite and Postgres alike.

use sea_orm_migration::prelude::*;

pub struct Migration;

impl MigrationName for Migration {
    fn name(&self) -> &str {
        "m0005_sync_dedup"
    }
}

#[derive(DeriveIden)]
enum Games {
    Table,
    DatabaseId,
    SourceRef,
}

#[derive(DeriveIden)]
enum SyncCursors {
    Table,
    Id,
    DatabaseId,
    Source,
    LastMonth,
    LastGameMs,
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // Stable provider game key (Lichess/Chess.com permalink). NULL for games
        // without one (manual uploads), and many NULLs are allowed since NULL is
        // distinct in a unique index on both backends — so only keyed games dedup.
        manager
            .alter_table(
                Table::alter()
                    .table(Games::Table)
                    .add_column(ColumnDef::new(Games::SourceRef).string().null())
                    .to_owned(),
            )
            .await?;

        // One game per (database, source_ref): the backstop that makes a re-sync
        // revisiting the cursor month/second safe (ingest also checks first).
        manager
            .create_index(
                Index::create()
                    .name("idx_games_database_source_ref")
                    .table(Games::Table)
                    .col(Games::DatabaseId)
                    .col(Games::SourceRef)
                    .unique()
                    .to_owned(),
            )
            .await?;

        manager
            .create_table(
                Table::create()
                    .table(SyncCursors::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(SyncCursors::Id)
                            .integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(SyncCursors::DatabaseId).integer().not_null())
                    .col(ColumnDef::new(SyncCursors::Source).string().not_null())
                    // Archive-based resume (Chess.com): last fully-synced "YYYY/MM".
                    .col(ColumnDef::new(SyncCursors::LastMonth).string().null())
                    // Stream-based resume (Lichess): epoch-ms of the newest game.
                    .col(ColumnDef::new(SyncCursors::LastGameMs).big_integer().null())
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_sync_cursors_database")
                            .from(SyncCursors::Table, SyncCursors::DatabaseId)
                            .to(Alias::new("databases"), Alias::new("id")),
                    )
                    .to_owned(),
            )
            .await?;

        // One cursor row per (database, source).
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

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(SyncCursors::Table).to_owned())
            .await?;
        manager
            .drop_index(
                Index::drop()
                    .name("idx_games_database_source_ref")
                    .table(Games::Table)
                    .to_owned(),
            )
            .await?;
        manager
            .alter_table(
                Table::alter()
                    .table(Games::Table)
                    .drop_column(Games::SourceRef)
                    .to_owned(),
            )
            .await?;
        Ok(())
    }
}
