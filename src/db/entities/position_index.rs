//! The Zobrist position index (ADR-0003): one row per indexed mainline ply,
//! answering "which games reached this position?" via an indexed `zobrist` lookup.

use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "position_index")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i64,
    /// The 64-bit Polyglot Zobrist key from `position::zobrist_of_fen`, stored
    /// as `i64`. Neither SQLite nor Postgres has an unsigned 64-bit integer, so
    /// the `u64` is reinterpreted bit-for-bit (`u64 as i64`) — lossless and
    /// reversible. Use [`to_i64`]/[`from_i64`] rather than casting inline.
    pub zobrist: i64,
    pub game_id: i32,
    /// 0-based half-move index of the position within the game's mainline.
    pub ply: i32,
    /// SAN of the move played from this position.
    #[sea_orm(column_name = "move")]
    pub r#move: String,
    /// Denormalized `databases.id` so position search can filter by scope and
    /// honor each database's `index_depth` without joining `games`.
    pub database_id: i32,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    /// Each indexed ply belongs to the game it was replayed from; enables joining
    /// the game's `result` in position search instead of a second id round-trip.
    #[sea_orm(
        belongs_to = "super::games::Entity",
        from = "Column::GameId",
        to = "super::games::Column::Id"
    )]
    Game,
}

impl ActiveModelBehavior for ActiveModel {}

/// Reinterpret a Zobrist `u64` as the `i64` stored in `position_index.zobrist`
/// (bit-preserving — the two's-complement encoding round-trips losslessly).
pub const fn to_i64(zobrist: u64) -> i64 {
    zobrist as i64
}

/// Inverse of [`to_i64`]: recover the original Zobrist `u64` from storage.
pub const fn from_i64(stored: i64) -> u64 {
    stored as u64
}
