//! Admin service-token minting (ADR-0044, issue #193): the only way to create
//! a scoped (`full` | `read_only` | `global_read`) service token server-side —
//! previously the only service token in existence was the auto-seeded local
//! one, with no route or CLI to mint another. Mirrors
//! [`ProviderService`](crate::ai::providers::ProviderService)'s shape.

use chrono::Utc;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, DbErr, EntityTrait, QueryFilter, Set,
};
use serde::Serialize;

use crate::db::entities::service_tokens;
use crate::server::auth::{
    new_token, SERVICE_SCOPE_FULL, SERVICE_SCOPE_GLOBAL_READ, SERVICE_SCOPE_READ_ONLY,
};
use crate::server::identity::{assert_admin, AuthError, CurrentUser};

/// A service token without its secret — the shape `list` returns.
#[derive(Debug, Clone, Serialize)]
pub struct ServiceTokenView {
    pub id: String,
    pub owner_id: String,
    pub is_admin: bool,
    pub label: String,
    pub scope: String,
    pub created_at: chrono::NaiveDateTime,
    pub expires_at: Option<chrono::NaiveDateTime>,
}

impl From<service_tokens::Model> for ServiceTokenView {
    fn from(m: service_tokens::Model) -> Self {
        Self {
            id: m.id,
            owner_id: m.owner_id,
            is_admin: m.is_admin,
            label: m.label,
            scope: m.scope,
            created_at: m.created_at,
            expires_at: m.expires_at,
        }
    }
}

/// The one-time response to [`ServiceTokenService::create`] — the only moment
/// the raw secret is ever shown.
#[derive(Debug, Clone, Serialize)]
pub struct MintedServiceToken {
    pub token: String,
    #[serde(flatten)]
    pub view: ServiceTokenView,
}

/// Why a service-token operation failed. Transport-agnostic.
#[derive(Debug, thiserror::Error)]
pub enum ServiceTokenError {
    #[error("not permitted")]
    Forbidden,
    #[error("service token not found")]
    NotFound,
    #[error("{0}")]
    InvalidInput(&'static str),
    #[error(transparent)]
    Db(#[from] DbErr),
}

impl From<AuthError> for ServiceTokenError {
    fn from(_: AuthError) -> Self {
        ServiceTokenError::Forbidden
    }
}

/// Admin-only CRUD over `service_tokens`. Unlike [`crate::ai::providers`]'s
/// per-owner scoping, service tokens are a single global admin operation —
/// server mode has one admin tier, not per-owner token management.
#[derive(Clone)]
pub struct ServiceTokenService {
    db: DatabaseConnection,
}

impl ServiceTokenService {
    pub fn new(db: DatabaseConnection) -> Self {
        Self { db }
    }

    /// Mint a fresh scoped token for `owner_id`. Admin-only; `scope` must be
    /// one of the three recognized values.
    #[allow(clippy::too_many_arguments)]
    pub async fn create(
        &self,
        admin: &CurrentUser,
        owner_id: &str,
        label: &str,
        scope: &str,
        is_admin: bool,
        expires_in_days: Option<i64>,
    ) -> Result<MintedServiceToken, ServiceTokenError> {
        assert_admin(admin)?;
        if !matches!(
            scope,
            SERVICE_SCOPE_FULL | SERVICE_SCOPE_READ_ONLY | SERVICE_SCOPE_GLOBAL_READ
        ) {
            return Err(ServiceTokenError::InvalidInput(
                "scope must be one of full, read_only, global_read",
            ));
        }
        if label.trim().is_empty() {
            return Err(ServiceTokenError::InvalidInput("label must not be empty"));
        }
        if owner_id.trim().is_empty() {
            return Err(ServiceTokenError::InvalidInput(
                "owner_id must not be empty",
            ));
        }

        let token = new_token();
        let id = new_token();
        let now = Utc::now().naive_utc();
        let expires_at = expires_in_days.map(|days| now + chrono::Duration::days(days));

        let model = service_tokens::ActiveModel {
            token: Set(token.clone()),
            id: Set(id),
            owner_id: Set(owner_id.to_string()),
            is_admin: Set(is_admin),
            scope: Set(scope.to_string()),
            label: Set(label.to_string()),
            created_at: Set(now),
            expires_at: Set(expires_at),
        }
        .insert(&self.db)
        .await?;

        Ok(MintedServiceToken {
            token,
            view: ServiceTokenView::from(model),
        })
    }

    /// Every service token — an admin operation, not scoped to the caller.
    pub async fn list(
        &self,
        admin: &CurrentUser,
    ) -> Result<Vec<ServiceTokenView>, ServiceTokenError> {
        assert_admin(admin)?;
        let rows = service_tokens::Entity::find().all(&self.db).await?;
        Ok(rows.into_iter().map(ServiceTokenView::from).collect())
    }

    /// Revoke a token by its non-secret reference `id` (never the bearer
    /// secret itself).
    pub async fn revoke(&self, admin: &CurrentUser, id: &str) -> Result<(), ServiceTokenError> {
        assert_admin(admin)?;
        let row = service_tokens::Entity::find()
            .filter(service_tokens::Column::Id.eq(id))
            .one(&self.db)
            .await?
            .ok_or(ServiceTokenError::NotFound)?;
        service_tokens::Entity::delete_by_id(row.token)
            .exec(&self.db)
            .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests;
