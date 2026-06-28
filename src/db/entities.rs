//! SeaORM entities for the core domain: the ownable `databases` collections plus
//! `games` + their `players`/`events` headers, the Zobrist `position_index`
//! (ADR-0003) and `studies` (a serialized `pgn_tree::MoveTree`). `settings` is the
//! scaffold key/value store. Relations are left empty (`enum Relation {}`) — joins
//! are issued explicitly by the query layer, mirroring the existing style.

/// Key/value application + user settings.
pub mod settings {
    use sea_orm::entity::prelude::*;

    #[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
    #[sea_orm(table_name = "settings")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub key: String,
        pub value: String,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

/// A named, ownable collection of games. `owner_id == NULL` means a global
/// (admin-managed) database searchable by every user.
pub mod databases {
    use sea_orm::entity::prelude::*;

    #[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
    #[sea_orm(table_name = "databases")]
    pub struct Model {
        #[sea_orm(primary_key)]
        pub id: i32,
        /// `None` ⇒ global/admin-owned, searchable by all users.
        pub owner_id: Option<String>,
        pub name: String,
        /// `lichess` | `chesscom` | `master` | `own`.
        pub kind: String,
        /// Position-index depth policy (ADR-0003): `None` ⇒ index every ply;
        /// `Some(n)` ⇒ cap `position_index` population to the first `n` plies.
        pub index_depth: Option<i32>,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}

    /// Default `index_depth` cap for a `master`/global database — the opening and
    /// early middlegame, where opening-explorer value concentrates (ADR-0003).
    pub const MASTER_INDEX_DEPTH: i32 = 36;

    /// The `index_depth` a freshly created database of the given `kind` should get:
    /// `Some(MASTER_INDEX_DEPTH)` for `master` (capped), `None` (full per-ply
    /// indexing) for a user's own `lichess`/`chesscom`/`own` databases.
    pub fn default_index_depth(kind: &str) -> Option<i32> {
        match kind {
            "master" => Some(MASTER_INDEX_DEPTH),
            _ => None,
        }
    }
}

/// A distinct player name (the `White`/`Black` PGN tags, deduplicated).
pub mod players {
    use sea_orm::entity::prelude::*;

    #[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
    #[sea_orm(table_name = "players")]
    pub struct Model {
        #[sea_orm(primary_key)]
        pub id: i32,
        #[sea_orm(unique)]
        pub name: String,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

/// A distinct event/tournament name (the `Event` PGN tag, deduplicated).
pub mod events {
    use sea_orm::entity::prelude::*;

    #[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
    #[sea_orm(table_name = "events")]
    pub struct Model {
        #[sea_orm(primary_key)]
        pub id: i32,
        #[sea_orm(unique)]
        pub name: String,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

/// A single game with its PGN header roster. `variant`/`start_fen` make Chess960
/// and set-up positions first-class (ADR-0010). Partial PGN dates (`1992.??.??`)
/// are kept verbatim as text, so `date` is a `String`, not a typed date.
pub mod games {
    use sea_orm::entity::prelude::*;

    #[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
    #[sea_orm(table_name = "games")]
    pub struct Model {
        #[sea_orm(primary_key)]
        pub id: i32,
        /// The collection this game belongs to (`databases.id`).
        pub database_id: i32,
        pub white_player_id: Option<i32>,
        pub black_player_id: Option<i32>,
        pub event_id: Option<i32>,
        pub site: Option<String>,
        pub round: Option<String>,
        /// PGN `Date` tag, kept verbatim (may be partial, e.g. `1992.??.??`).
        pub date: Option<String>,
        /// `1-0` | `0-1` | `1/2-1/2` | `*`.
        pub result: Option<String>,
        /// ECO opening code (e.g. `B90`).
        pub eco: Option<String>,
        pub white_elo: Option<i32>,
        pub black_elo: Option<i32>,
        /// Chess variant (ADR-0010); `standard` unless overridden.
        pub variant: String,
        /// Non-standard start position (Chess960 / set-up). `None` ⇒ the standard
        /// startpos for `variant`.
        pub start_fen: Option<String>,
        pub ply_count: Option<i32>,
        /// PGN movetext (mainline SAN plus any variations/comments).
        pub pgn: Option<String>,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

/// The Zobrist position index (ADR-0003): one row per indexed mainline ply,
/// answering "which games reached this position?" via an indexed `zobrist` lookup.
pub mod position_index {
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
    pub enum Relation {}

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
}

/// A study: a named, serialized `pgn_tree::MoveTree` (variations, comments, NAGs)
/// living in a database. `owner_id == NULL` mirrors the global-collection rule.
pub mod studies {
    use sea_orm::entity::prelude::*;

    #[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
    #[sea_orm(table_name = "studies")]
    pub struct Model {
        #[sea_orm(primary_key)]
        pub id: i32,
        pub database_id: i32,
        /// `None` ⇒ global/admin-owned, like `databases.owner_id`.
        pub owner_id: Option<String>,
        pub name: String,
        /// The `pgn_tree::MoveTree` serialized as JSON.
        pub tree_json: String,
        pub created_at: DateTime,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

#[cfg(test)]
mod tests {
    use super::{databases, position_index};

    #[test]
    fn zobrist_cast_round_trips() {
        // High-bit-set value would overflow a naive i64 conversion; the bitwise
        // reinterpret must round-trip it.
        for z in [
            0u64,
            1,
            u64::MAX,
            0x8000_0000_0000_0000,
            0xDEAD_BEEF_CAFE_F00D,
        ] {
            assert_eq!(position_index::from_i64(position_index::to_i64(z)), z);
        }
    }

    #[test]
    fn index_depth_defaults_by_kind() {
        assert_eq!(
            databases::default_index_depth("master"),
            Some(databases::MASTER_INDEX_DEPTH)
        );
        assert_eq!(databases::default_index_depth("own"), None);
        assert_eq!(databases::default_index_depth("lichess"), None);
        assert_eq!(databases::default_index_depth("chesscom"), None);
    }
}
