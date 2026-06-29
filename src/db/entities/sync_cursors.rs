//! Persisted incremental-sync position, one row per `(database_id, source)`
//! (issue #95). Mirrors `collectors::SyncCursor`: archive-based sources resume
//! from `last_month`, stream-based sources from `last_game_ms`.

use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "sync_cursors")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    /// The collection this cursor tracks (`databases.id`).
    pub database_id: i32,
    /// Provider tag: `lichess` | `chesscom`.
    pub source: String,
    /// Last fully-synced month, `"YYYY/MM"` (Chess.com).
    pub last_month: Option<String>,
    /// Epoch-ms of the most recently synced game (Lichess).
    pub last_game_ms: Option<i64>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
