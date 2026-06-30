//! An AI study-assistant chat session (issue #20). Scoped to its `owner_id`
//! (like `studies.owner_id` — a plain owner string, `local-admin` in local mode)
//! and carrying the model id its agent loop drives.

use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "assistant_sessions")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    /// Matches the caller's `CurrentUser::id`; only the owner may read/post.
    pub owner_id: String,
    pub title: String,
    /// The LLM model id the loop sends to the provider for this session.
    pub model: String,
    pub created_at: DateTime,
    pub updated_at: DateTime,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
