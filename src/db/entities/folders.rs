//! A folder: a node in the adjacency-list directory tree that organizes studies
//! (issue #164). `owner_id == NULL` is a global/admin folder; `parent_id == NULL`
//! is a root folder. The tree is account-level, independent of game databases.

use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "folders")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    /// `None` ⇒ global/admin-owned, like `studies.owner_id`.
    pub owner_id: Option<String>,
    /// `None` ⇒ a root folder; otherwise the id of the containing folder.
    pub parent_id: Option<i32>,
    pub name: String,
    pub created_at: DateTime,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
