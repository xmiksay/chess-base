//! One row of the append-only entanglement agent event log (#198/m0008). Events
//! are strictly ordered by `ord` within their `root_session_id` (unique
//! together); `payload` is the engine's serialized event JSON.

use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "agent_events")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i64,
    /// The engine conversation this event belongs to (`agent_sessions` PK).
    pub root_session_id: String,
    pub user_id: String,
    /// Monotonic order within the root session (the log is replayed by `ord`).
    pub ord: i64,
    /// Event timestamp, unix milliseconds (engine-supplied).
    pub ts: i64,
    /// The (possibly nested sub-agent) session that produced the event.
    pub session_id: String,
    /// `in` (to the engine) or `out` (from the engine).
    pub direction: String,
    /// The serialized engine event.
    pub payload: String,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
