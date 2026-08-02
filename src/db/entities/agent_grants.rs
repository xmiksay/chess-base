//! A persisted per-user tool approval for the entanglement agent engine
//! (#198/m0008): "always allow `tool`" — optionally narrowed to one argument
//! shape (`arg`) — with `scope` naming how wide the grant applies. Unique per
//! `(user_id, tool, arg)`.

use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "agent_grants")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    /// Matches the caller's `CurrentUser::id`.
    pub user_id: String,
    /// The MCP tool name the grant covers.
    pub tool: String,
    /// Optional argument the grant is narrowed to; `None` ⇒ any arguments.
    pub arg: Option<String>,
    pub scope: String,
    pub created_at: DateTime,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
