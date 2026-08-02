//! Persisted incremental-sync position, one row per `(database_id, source,
//! username)` (issue #95, username-keyed since #197). Mirrors
//! `collectors::SyncCursor`: archive-based sources resume from `last_month`,
//! stream-based sources from `last_game_ms`. `last_synced_at`/`last_imported`/
//! `last_duplicates` (#197) record the most recent run so the UI can show real
//! feedback instead of nothing.

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
    /// Provider username this cursor tracks. `NULL` only on pre-#197 rows
    /// (never matched by a current load, which always passes a username).
    pub username: Option<String>,
    /// Last fully-synced month, `"YYYY/MM"` (Chess.com).
    pub last_month: Option<String>,
    /// Epoch-ms of the most recently synced game (Lichess).
    pub last_game_ms: Option<i64>,
    /// When the most recent sync run (success or failure) last saved this cursor.
    pub last_synced_at: Option<DateTime>,
    /// Games imported by the most recent sync run.
    pub last_imported: Option<i32>,
    /// Games dropped as already present by the most recent sync run.
    pub last_duplicates: Option<i32>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
