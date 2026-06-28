//! A registered OAuth 2.1 client (ADR-0016). claude.ai self-onboards via dynamic
//! client registration (RFC 7591), so a client is created on first contact with
//! the redirect URIs it declares. Public clients only — authentication is PKCE,
//! never a client secret.

use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "oauth_clients")]
pub struct Model {
    /// Issued client identifier (random, unguessable).
    #[sea_orm(primary_key, auto_increment = false)]
    pub client_id: String,
    pub client_name: String,
    /// JSON-encoded array of the client's allowed redirect URIs.
    pub redirect_uris: String,
    pub created_at: DateTime,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
