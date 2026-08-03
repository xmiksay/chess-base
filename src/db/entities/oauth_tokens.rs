//! An issued OAuth 2.1 token pair (ADR-0016, hardened in ADR-0044). The
//! `access_token` is the bearer `authenticate_mcp` resolves on every `/mcp`
//! call; the `refresh_token` mints a fresh pair once the access token expires.
//! Both rotate on refresh — but rotation now **revokes the old row in place**
//! instead of deleting it (`revoked`), so a replayed already-rotated-away
//! refresh token can be detected as reuse and the whole `family_id` revoked.

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
    /// Groups every row descended from one authorization-code exchange across
    /// rotations (ADR-0044) — the reuse-detection unit: replaying a revoked
    /// row's refresh token revokes every row sharing this id.
    pub family_id: String,
    /// Set on rotation instead of deleting the row (ADR-0044): a revoked row's
    /// refresh token being presented again is reuse, so `authenticate_mcp`
    /// treats `revoked` the same as an expired access token.
    pub revoked: bool,
    pub created_at: DateTime,
    /// Hard expiry of the access token; refresh tokens are additionally capped
    /// by the family's absolute lifetime (`ABSOLUTE_REFRESH_TTL_DAYS`).
    pub expires_at: DateTime,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
