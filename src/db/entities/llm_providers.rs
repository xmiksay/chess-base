//! A configured LLM provider (issue #20, extended for the entanglement agent
//! engine in #198/m0008). `api_key` is **server-side only** — it is never
//! serialized to the SPA (the provider DTOs omit it).

use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "llm_providers")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    /// Display name, unique per owner (e.g. `anthropic`).
    pub name: String,
    /// Default model id used by sessions that don't override it.
    pub model: String,
    /// Secret API key — never returned to clients.
    pub api_key: String,
    /// At most one row should be the default; the resolver takes the first.
    pub is_default: bool,
    pub created_at: DateTime,
    /// Owning user; `None` ⇒ an admin-managed global row.
    pub owner_id: Option<String>,
    /// Wire protocol the provider speaks (e.g. `anthropic`, `openai`).
    pub wire: String,
    /// Endpoint override; `None` ⇒ the wire's default endpoint.
    pub base_url: Option<String>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
