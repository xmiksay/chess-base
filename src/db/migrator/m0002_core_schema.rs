//! m0002 — core domain schema: `players`, `events`, `games`, the Zobrist
//! `position_index` (ADR-0003) and `studies`, plus the `databases.index_depth`
//! policy column. Schema-builder only, so it runs on SQLite and Postgres alike.

use sea_orm_migration::prelude::*;

pub struct Migration;

impl MigrationName for Migration {
    fn name(&self) -> &str {
        "m0002_core_schema"
    }
}

#[derive(DeriveIden)]
enum Databases {
    Table,
    IndexDepth,
}

#[derive(DeriveIden)]
enum Players {
    Table,
    Id,
    Name,
}

#[derive(DeriveIden)]
enum Events {
    Table,
    Id,
    Name,
}

#[derive(DeriveIden)]
enum Games {
    Table,
    Id,
    DatabaseId,
    WhitePlayerId,
    BlackPlayerId,
    EventId,
    Site,
    Round,
    Date,
    Result,
    Eco,
    WhiteElo,
    BlackElo,
    Variant,
    StartFen,
    PlyCount,
    Pgn,
}

#[derive(DeriveIden)]
enum PositionIndex {
    Table,
    Id,
    Zobrist,
    GameId,
    Ply,
    Move,
    DatabaseId,
}

#[derive(DeriveIden)]
enum Studies {
    Table,
    Id,
    DatabaseId,
    OwnerId,
    Name,
    TreeJson,
    CreatedAt,
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // `index_depth`: NULL ⇒ full per-ply indexing (own DBs); a number caps the
        // position index to the first N plies (master/global DBs). The per-`kind`
        // default is applied in code on insert (entities::databases).
        manager
            .alter_table(
                Table::alter()
                    .table(Databases::Table)
                    .add_column(ColumnDef::new(Databases::IndexDepth).integer().null())
                    .to_owned(),
            )
            .await?;

        manager
            .create_table(
                Table::create()
                    .table(Players::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(Players::Id)
                            .integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(Players::Name)
                            .string()
                            .not_null()
                            .unique_key(),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_table(
                Table::create()
                    .table(Events::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(Events::Id)
                            .integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(Events::Name)
                            .string()
                            .not_null()
                            .unique_key(),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_table(
                Table::create()
                    .table(Games::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(Games::Id)
                            .integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(Games::DatabaseId).integer().not_null())
                    .col(ColumnDef::new(Games::WhitePlayerId).integer().null())
                    .col(ColumnDef::new(Games::BlackPlayerId).integer().null())
                    .col(ColumnDef::new(Games::EventId).integer().null())
                    .col(ColumnDef::new(Games::Site).string().null())
                    .col(ColumnDef::new(Games::Round).string().null())
                    .col(ColumnDef::new(Games::Date).string().null())
                    .col(ColumnDef::new(Games::Result).string().null())
                    .col(ColumnDef::new(Games::Eco).string().null())
                    .col(ColumnDef::new(Games::WhiteElo).integer().null())
                    .col(ColumnDef::new(Games::BlackElo).integer().null())
                    .col(
                        ColumnDef::new(Games::Variant)
                            .string()
                            .not_null()
                            .default("standard"),
                    )
                    .col(ColumnDef::new(Games::StartFen).string().null())
                    .col(ColumnDef::new(Games::PlyCount).integer().null())
                    .col(ColumnDef::new(Games::Pgn).text().null())
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_games_database")
                            .from(Games::Table, Games::DatabaseId)
                            .to(Databases::Table, Alias::new("id")),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_games_white_player")
                            .from(Games::Table, Games::WhitePlayerId)
                            .to(Players::Table, Players::Id),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_games_black_player")
                            .from(Games::Table, Games::BlackPlayerId)
                            .to(Players::Table, Players::Id),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_games_event")
                            .from(Games::Table, Games::EventId)
                            .to(Events::Table, Events::Id),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_table(
                Table::create()
                    .table(PositionIndex::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(PositionIndex::Id)
                            .big_integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    // Zobrist u64 stored bit-for-bit as i64 (see entities::position_index).
                    .col(
                        ColumnDef::new(PositionIndex::Zobrist)
                            .big_integer()
                            .not_null(),
                    )
                    .col(ColumnDef::new(PositionIndex::GameId).integer().not_null())
                    .col(ColumnDef::new(PositionIndex::Ply).integer().not_null())
                    .col(ColumnDef::new(PositionIndex::Move).string().not_null())
                    .col(
                        ColumnDef::new(PositionIndex::DatabaseId)
                            .integer()
                            .not_null(),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_position_index_game")
                            .from(PositionIndex::Table, PositionIndex::GameId)
                            .to(Games::Table, Games::Id),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_position_index_database")
                            .from(PositionIndex::Table, PositionIndex::DatabaseId)
                            .to(Databases::Table, Alias::new("id")),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_table(
                Table::create()
                    .table(Studies::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(Studies::Id)
                            .integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(Studies::DatabaseId).integer().not_null())
                    .col(ColumnDef::new(Studies::OwnerId).string().null())
                    .col(ColumnDef::new(Studies::Name).string().not_null())
                    .col(ColumnDef::new(Studies::TreeJson).text().not_null())
                    .col(
                        ColumnDef::new(Studies::CreatedAt)
                            .timestamp()
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_studies_database")
                            .from(Studies::Table, Studies::DatabaseId)
                            .to(Databases::Table, Alias::new("id")),
                    )
                    .to_owned(),
            )
            .await?;

        // Indices: position lookup by zobrist, header filters, and database scoping.
        let indices = [
            index_on(
                PositionIndex::Table,
                PositionIndex::Zobrist,
                "idx_position_index_zobrist",
            ),
            index_on(
                PositionIndex::Table,
                PositionIndex::GameId,
                "idx_position_index_game",
            ),
            index_on(
                PositionIndex::Table,
                PositionIndex::DatabaseId,
                "idx_position_index_database",
            ),
            index_on(Games::Table, Games::DatabaseId, "idx_games_database"),
            index_on(Games::Table, Games::WhitePlayerId, "idx_games_white_player"),
            index_on(Games::Table, Games::BlackPlayerId, "idx_games_black_player"),
            index_on(Games::Table, Games::EventId, "idx_games_event"),
            index_on(Games::Table, Games::Date, "idx_games_date"),
            index_on(Games::Table, Games::Eco, "idx_games_eco"),
            index_on(Games::Table, Games::Result, "idx_games_result"),
            index_on(Studies::Table, Studies::DatabaseId, "idx_studies_database"),
            index_on(Studies::Table, Studies::OwnerId, "idx_studies_owner"),
        ];
        for index in indices {
            manager.create_index(index).await?;
        }

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // Drop in FK-dependency order; dropping a table removes its indices.
        for table in [
            Table::drop().table(PositionIndex::Table).to_owned(),
            Table::drop().table(Studies::Table).to_owned(),
            Table::drop().table(Games::Table).to_owned(),
            Table::drop().table(Events::Table).to_owned(),
            Table::drop().table(Players::Table).to_owned(),
        ] {
            manager.drop_table(table).await?;
        }

        manager
            .alter_table(
                Table::alter()
                    .table(Databases::Table)
                    .drop_column(Databases::IndexDepth)
                    .to_owned(),
            )
            .await?;

        Ok(())
    }
}

/// A single-column, non-unique index named `name` on `table.column`.
fn index_on<T, C>(table: T, column: C, name: &str) -> IndexCreateStatement
where
    T: IntoIden + 'static,
    C: IntoIndexColumn,
{
    Index::create()
        .name(name)
        .table(table)
        .col(column)
        .to_owned()
}
