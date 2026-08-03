//! m0011 — public sharing flags (issue #211, ADR-0045).
//!
//! `games.public` and `studies.public`: independent per-object booleans that
//! opt a game or study into the anonymous HTTP read tier (plain deep links, no
//! share tokens). Default `false` — nothing existing becomes public. No
//! indexes: reads are by primary key, and the studies read-scope filter rides
//! on the existing owner scan.
//!
//! Schema-builder only (one ADD COLUMN per `alter_table` call, SQLite-safe),
//! so the same migration runs on SQLite and Postgres.

use sea_orm_migration::prelude::*;

pub struct Migration;

impl MigrationName for Migration {
    fn name(&self) -> &str {
        "m0011_sharing"
    }
}

#[derive(DeriveIden)]
enum Games {
    Table,
    Public,
}

#[derive(DeriveIden)]
enum Studies {
    Table,
    Public,
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(Games::Table)
                    .add_column(
                        ColumnDef::new(Games::Public)
                            .boolean()
                            .not_null()
                            .default(false),
                    )
                    .to_owned(),
            )
            .await?;
        manager
            .alter_table(
                Table::alter()
                    .table(Studies::Table)
                    .add_column(
                        ColumnDef::new(Studies::Public)
                            .boolean()
                            .not_null()
                            .default(false),
                    )
                    .to_owned(),
            )
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(Studies::Table)
                    .drop_column(Studies::Public)
                    .to_owned(),
            )
            .await?;
        manager
            .alter_table(
                Table::alter()
                    .table(Games::Table)
                    .drop_column(Games::Public)
                    .to_owned(),
            )
            .await?;
        Ok(())
    }
}
