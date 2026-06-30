//! An admin-configured LLM provider (issue #20). The default row (`is_default`)
//! builds the [`LlmProvider`] at startup, taking precedence over the
//! `ANTHROPIC_API_KEY` env fallback. `api_key` is **server-side only** — it is
//! never serialized to the SPA (the provider DTOs omit it).
//!
//! [`LlmProvider`]: crate::ai::llm::LlmProvider

use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "llm_providers")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    /// Unique display name (e.g. `anthropic`).
    pub name: String,
    /// Default model id used by sessions that don't override it.
    pub model: String,
    /// Secret API key — never returned to clients.
    pub api_key: String,
    /// At most one row should be the default; the resolver takes the first.
    pub is_default: bool,
    pub created_at: DateTime,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
