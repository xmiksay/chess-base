//! An issued OAuth 2.1 token pair (ADR-0016). The `access_token` is the bearer
//! `authenticate_mcp` resolves on every `/mcp` call; the `refresh_token` mints a
//! fresh pair once the access token expires. Both rotate on refresh.

use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "oauth_tokens")]
pub struct Model {
    /// Short-lived bearer presented at `/mcp`.
    #[sea_orm(primary_key, auto_increment = false)]
    pub access_token: String,
    /// Long-lived secret exchanged for a new pair at `/oauth/token`.
    #[sea_orm(unique)]
    pub refresh_token: String,
    pub client_id: String,
    /// The user this token acts as; lands in the ownership `scope` filter.
    pub user_id: String,
    pub scope: String,
    pub created_at: DateTime,
    /// Hard expiry of the access token; refresh tokens are checked against the
    /// row's existence (revocation deletes the row).
    pub expires_at: DateTime,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
