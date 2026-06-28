//! A static MCP bearer token (ADR-0016). Unlike a `sessions` row it never came
//! from a password login: in local mode one row is seeded for the implicit
//! `local-admin` and printed at startup; in server mode an admin may issue
//! long-lived tokens for headless clients. `owner_id` lands in the ownership
//! `scope` filter just like `users.id`, and `is_admin` carries the role so the
//! token resolves to a full [`CurrentUser`] without a `users` lookup.
//!
//! [`CurrentUser`]: crate::server::identity::CurrentUser

use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "service_tokens")]
pub struct Model {
    /// The bearer secret itself (random, unguessable).
    #[sea_orm(primary_key, auto_increment = false)]
    pub token: String,
    /// Owner key stamped onto resources this token creates/edits.
    pub owner_id: String,
    /// Whether the token may manage global (`owner_id IS NULL`) resources.
    pub is_admin: bool,
    /// Human label shown when listing tokens (e.g. `"local"`).
    pub label: String,
    pub created_at: DateTime,
    /// Optional hard expiry; `None` ⇒ never expires.
    pub expires_at: Option<DateTime>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
