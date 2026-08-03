//! A pending OAuth consent decision (ADR-0044, issue #193): `GET
//! /oauth/authorize` inserts one of these instead of minting a code directly
//! when the user hasn't previously approved this client. Single-use and
//! short-lived (10 minutes) — `csrf_token` is both its primary key and the
//! anti-CSRF token bound to this specific request: it is delivered only inside
//! the rendered `GET /oauth/consent` page, so a forged cross-site
//! `POST /oauth/consent` cannot supply it.

use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "oauth_consent_requests")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub csrf_token: String,
    pub user_id: String,
    pub client_id: String,
    pub redirect_uri: String,
    pub code_challenge: String,
    pub code_challenge_method: String,
    pub scope: String,
    pub state: Option<String>,
    pub expires_at: DateTime,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
