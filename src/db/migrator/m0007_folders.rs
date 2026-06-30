//! m0007 — study folder hierarchy + game-linked analyses (issue #164, ADR-0030).
//!
//! - `folders` — an adjacency-list directory tree for organizing studies,
//!   account-level and independent of game databases. `owner_id IS NULL` is a
//!   global/admin folder (mirrors `studies`/`databases`, ADR 0007/0011);
//!   `parent_id IS NULL` is a root folder. A self-referential FK cascades child
//!   folders on Postgres; SQLite (FKs off by default, and no ALTER-add support)
//!   relies on `FolderService` to cascade in the app layer.
//! - `studies` gains `folder_id` (which folder it lives in; NULL = unfiled/root)
//!   and `origin_game_id` (the game an analysis was built from; NULL = standalone).
//!   Added as plain nullable columns — SQLite cannot ALTER-add a foreign key — so
//!   referential cleanup (folder delete → NULL) is enforced in the service layer.
//!
//! Schema-builder only, so the same migration runs on SQLite and Postgres.

use sea_orm_migration::prelude::*;

pub struct Migration;

impl MigrationName for Migration {
    fn name(&self) -> &str {
        "m0007_folders"
    }
}

#[derive(DeriveIden)]
enum Folders {
    Table,
    Id,
    OwnerId,
    ParentId,
    Name,
    CreatedAt,
}

#[derive(DeriveIden)]
enum Studies {
    Table,
    FolderId,
    OriginGameId,
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(Folders::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(Folders::Id)
                            .integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    // NULL ⇒ global/admin folder (mirrors studies/databases).
                    .col(ColumnDef::new(Folders::OwnerId).string().null())
                    // NULL ⇒ root folder; otherwise a child of another folder.
                    .col(ColumnDef::new(Folders::ParentId).integer().null())
                    .col(ColumnDef::new(Folders::Name).string().not_null())
                    .col(
                        ColumnDef::new(Folders::CreatedAt)
                            .timestamp()
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_folders_parent")
                            .from(Folders::Table, Folders::ParentId)
                            .to(Folders::Table, Folders::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;

        // No duplicate sibling names within one owner's directory level. NULL is
        // distinct in a unique index on both backends, so global roots and a
        // user's roots don't collide with each other.
        for index in [
            Index::create()
                .name("idx_folders_owner_parent_name")
                .table(Folders::Table)
                .col(Folders::OwnerId)
                .col(Folders::ParentId)
                .col(Folders::Name)
                .unique()
                .to_owned(),
            Index::create()
                .name("idx_folders_parent")
                .table(Folders::Table)
                .col(Folders::ParentId)
                .to_owned(),
            Index::create()
                .name("idx_folders_owner")
                .table(Folders::Table)
                .col(Folders::OwnerId)
                .to_owned(),
        ] {
            manager.create_index(index).await?;
        }

        // An independent organization axis (which folder) plus the game an
        // analysis was built from. Plain nullable columns: SQLite can't ALTER-add
        // a foreign key, so the service enforces the SET NULL on folder delete.
        // One column per ALTER — SQLite rejects multiple options in one statement.
        manager
            .alter_table(
                Table::alter()
                    .table(Studies::Table)
                    .add_column(ColumnDef::new(Studies::FolderId).integer().null())
                    .to_owned(),
            )
            .await?;
        manager
            .alter_table(
                Table::alter()
                    .table(Studies::Table)
                    .add_column(ColumnDef::new(Studies::OriginGameId).integer().null())
                    .to_owned(),
            )
            .await?;

        for index in [
            Index::create()
                .name("idx_studies_folder")
                .table(Studies::Table)
                .col(Studies::FolderId)
                .to_owned(),
            Index::create()
                .name("idx_studies_origin_game")
                .table(Studies::Table)
                .col(Studies::OriginGameId)
                .to_owned(),
        ] {
            manager.create_index(index).await?;
        }

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        for index in [
            Index::drop()
                .name("idx_studies_origin_game")
                .table(Studies::Table)
                .to_owned(),
            Index::drop()
                .name("idx_studies_folder")
                .table(Studies::Table)
                .to_owned(),
        ] {
            manager.drop_index(index).await?;
        }
        manager
            .alter_table(
                Table::alter()
                    .table(Studies::Table)
                    .drop_column(Studies::OriginGameId)
                    .to_owned(),
            )
            .await?;
        manager
            .alter_table(
                Table::alter()
                    .table(Studies::Table)
                    .drop_column(Studies::FolderId)
                    .to_owned(),
            )
            .await?;
        manager
            .drop_table(Table::drop().table(Folders::Table).to_owned())
            .await?;
        Ok(())
    }
}
