//! One turn in an assistant session's transcript (issue #20). `content_json` is a
//! provider-agnostic `ai::llm::Message` serialized as JSON; `role` is lifted out
//! of it for cheap ordering/filtering (counting tool iterations, finding the last
//! user turn) without parsing every row.

use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "assistant_messages")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    pub session_id: i32,
    /// Monotonic order within the session (the transcript is replayed by `seq`).
    pub seq: i32,
    /// The serialized message's tag: `user` | `assistant` | `tool_results`.
    pub role: String,
    /// The `ai::llm::Message` serialized as JSON.
    pub content_json: String,
    pub created_at: DateTime,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
