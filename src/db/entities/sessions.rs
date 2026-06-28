//! An opaque server-mode session token. The same token backs both a `Bearer`
//! header and a browser `session` cookie; resolution looks the row up, checks
//! `expires_at`, and loads the owning user (see `auth::AuthService`).

use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "sessions")]
pub struct Model {
    /// The bearer/cookie secret itself (random, unguessable).
    #[sea_orm(primary_key, auto_increment = false)]
    pub token: String,
    /// FK to `users.id`; the session's owner.
    pub user_id: String,
    pub created_at: DateTime,
    /// Hard expiry; a session past this is treated as absent.
    pub expires_at: DateTime,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
