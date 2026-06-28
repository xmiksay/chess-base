//! A server-mode user account. `id` is the stable string that lands in
//! `databases.owner_id` / `studies.owner_id`, so it is what the ownership
//! `scope` filter matches. Local mode has no rows here — it uses the implicit
//! `local-admin` (see `server::identity`).

use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "users")]
pub struct Model {
    /// Opaque, stable id used as the owner key on ownable resources.
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: String,
    /// Unique login name.
    #[sea_orm(unique)]
    pub username: String,
    /// Argon2 PHC string (never the plaintext); not surfaced to clients.
    pub password_hash: String,
    /// Admins may create/manage global (`owner_id IS NULL`) databases & studies.
    pub is_admin: bool,
    pub created_at: DateTime,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
