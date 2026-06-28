//! A single game with its PGN header roster. `variant`/`start_fen` make Chess960
//! and set-up positions first-class (ADR-0010). Partial PGN dates (`1992.??.??`)
//! are kept verbatim as text, so `date` is a `String`, not a typed date.

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
