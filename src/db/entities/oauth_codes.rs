//! A short-lived OAuth 2.1 authorization code (ADR-0016). Issued by
//! `/oauth/authorize` for a logged-in, consenting user and redeemed once at
//! `/oauth/token` against the PKCE `code_verifier`. `used` guards against replay.

use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "oauth_codes")]
pub struct Model {
    /// The authorization code itself (random, unguessable).
    #[sea_orm(primary_key, auto_increment = false)]
    pub code: String,
    pub client_id: String,
    /// The authenticated user the code (and resulting tokens) act as.
    pub user_id: String,
    pub redirect_uri: String,
    /// PKCE `code_challenge` (base64url SHA-256 of the verifier).
    pub code_challenge: String,
    pub code_challenge_method: String,
    pub scope: String,
    /// Hard expiry; a code past this is rejected at the token endpoint.
    pub expires_at: DateTime,
    /// Set once redeemed so the code cannot be replayed.
    pub used: bool,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
