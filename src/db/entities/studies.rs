//! A study: a named, serialized `pgn_tree::MoveTree` (variations, comments, NAGs)
//! living in a database. `owner_id == NULL` mirrors the global-collection rule.

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
    /// Which folder this study lives in (issue #164). `None` ⇒ unfiled (root).
    /// No DB-level FK (SQLite can't ALTER-add one): `FolderService::delete`
    /// nulls this when the containing folder is removed.
    pub folder_id: Option<i32>,
    /// The game an analysis was built from (issue #164). `None` ⇒ standalone.
    pub origin_game_id: Option<i32>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
