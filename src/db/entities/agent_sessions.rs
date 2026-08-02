//! One entanglement agent engine conversation (#198/m0008), keyed by the
//! engine's `root_session_id`. Its transcript is the `agent_events` log.

use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "agent_sessions")]
pub struct Model {
    /// The engine-assigned root session id (string, not auto-increment).
    #[sea_orm(primary_key, auto_increment = false)]
    pub root_session_id: String,
    /// Matches the caller's `CurrentUser::id`; only the owner may read/post.
    pub user_id: String,
    /// User-visible title; `None` until the first message names it.
    pub name: Option<String>,
    /// The agent definition the session runs.
    pub agent: String,
    pub created_at: DateTime,
    pub last_active: DateTime,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
