//! m0001 — the scaffold schema: `settings` (key/value) and `databases` (the
//! ownable game collections). Schema-builder only, so it runs on SQLite and
//! Postgres alike.

use sea_orm_migration::prelude::*;

pub struct Migration;

impl MigrationName for Migration {
    fn name(&self) -> &str {
        "m0001_init"
    }
}

#[derive(DeriveIden)]
enum Settings {
    Table,
    Key,
    Value,
}

#[derive(DeriveIden)]
enum Databases {
    Table,
    Id,
    OwnerId,
    Name,
    Kind,
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(Settings::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(Settings::Key)
                            .string()
                            .not_null()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(Settings::Value).string().not_null())
                    .to_owned(),
            )
            .await?;

        manager
            .create_table(
                Table::create()
                    .table(Databases::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(Databases::Id)
                            .integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(Databases::OwnerId).string().null())
                    .col(ColumnDef::new(Databases::Name).string().not_null())
                    .col(ColumnDef::new(Databases::Kind).string().not_null())
                    .to_owned(),
            )
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(Databases::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(Settings::Table).to_owned())
            .await?;
        Ok(())
    }
}
