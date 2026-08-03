//! A remembered OAuth consent approval (ADR-0044, issue #193): once a user
//! approves a client on the consent screen, this row lets a later
//! `GET /oauth/authorize` for the same `(user_id, client_id)` pair skip the
//! screen and issue a code directly, mirroring the pre-hardening
//! implicit-consent behavior for every subsequent authorization.

use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "oauth_consents")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub user_id: String,
    #[sea_orm(primary_key, auto_increment = false)]
    pub client_id: String,
    pub created_at: DateTime,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
