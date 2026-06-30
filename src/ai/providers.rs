//! Admin-managed LLM provider registry (issue #20), backing the `llm_providers`
//! table. The **default** row builds the [`LlmProvider`] at startup, taking
//! precedence over the `ANTHROPIC_API_KEY` env fallback; with no row and no env
//! key the assistant and study-generation paths stay disabled.
//!
//! API keys are **server-side only**: [`ProviderService::list`] returns
//! [`ProviderInfo`] without the key, and the resolver consumes keys internally to
//! build a client — they never reach the SPA.

use std::sync::Arc;

use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, DbErr, EntityTrait, QueryFilter, QueryOrder,
    Set,
};

use crate::ai::llm::anthropic::AnthropicProvider;
use crate::ai::llm::LlmProvider;
use crate::db::entities::llm_providers;
use crate::server::identity::{assert_admin, AuthError, CurrentUser};

/// A provider config without its secret key — the only shape exposed to clients.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ProviderInfo {
    pub id: i32,
    pub name: String,
    pub model: String,
    pub is_default: bool,
}

impl From<llm_providers::Model> for ProviderInfo {
    fn from(m: llm_providers::Model) -> Self {
        Self {
            id: m.id,
            name: m.name,
            model: m.model,
            is_default: m.is_default,
        }
    }
}

/// Fields to create or update a provider.
pub struct ProviderInput {
    pub name: String,
    pub model: String,
    pub api_key: String,
    pub is_default: bool,
}

/// Why a provider operation failed. Transport-agnostic.
#[derive(Debug, thiserror::Error)]
pub enum ProviderStoreError {
    #[error("not permitted")]
    Forbidden,
    #[error("provider not found")]
    NotFound,
    #[error(transparent)]
    Db(#[from] DbErr),
}

impl From<AuthError> for ProviderStoreError {
    fn from(_: AuthError) -> Self {
        ProviderStoreError::Forbidden
    }
}

/// CRUD over `llm_providers` plus the startup [`resolve`](Self::resolve) that turns
/// the default row (or the env fallback) into a live [`LlmProvider`].
#[derive(Clone)]
pub struct ProviderService {
    db: DatabaseConnection,
}

impl ProviderService {
    pub fn new(db: DatabaseConnection) -> Self {
        Self { db }
    }

    /// All configured providers (no keys), newest first.
    pub async fn list(&self) -> Result<Vec<ProviderInfo>, ProviderStoreError> {
        let rows = llm_providers::Entity::find()
            .order_by_desc(llm_providers::Column::Id)
            .all(&self.db)
            .await?;
        Ok(rows.into_iter().map(ProviderInfo::from).collect())
    }

    /// Create or update a provider by name (admin only). Making this row the
    /// default clears the flag on every other row so at most one default remains.
    pub async fn upsert(
        &self,
        user: &CurrentUser,
        input: ProviderInput,
    ) -> Result<ProviderInfo, ProviderStoreError> {
        assert_admin(user)?;
        if input.is_default {
            self.clear_defaults().await?;
        }
        let existing = llm_providers::Entity::find()
            .filter(llm_providers::Column::Name.eq(input.name.clone()))
            .one(&self.db)
            .await?;
        let model = match existing {
            Some(row) => {
                let mut active: llm_providers::ActiveModel = row.into();
                active.model = Set(input.model);
                active.api_key = Set(input.api_key);
                active.is_default = Set(input.is_default);
                active.update(&self.db).await?
            }
            None => {
                llm_providers::ActiveModel {
                    name: Set(input.name),
                    model: Set(input.model),
                    api_key: Set(input.api_key),
                    is_default: Set(input.is_default),
                    ..Default::default()
                }
                .insert(&self.db)
                .await?
            }
        };
        Ok(ProviderInfo::from(model))
    }

    /// Delete a provider by id (admin only).
    pub async fn delete(&self, user: &CurrentUser, id: i32) -> Result<(), ProviderStoreError> {
        assert_admin(user)?;
        let res = llm_providers::Entity::delete_by_id(id)
            .exec(&self.db)
            .await?;
        if res.rows_affected == 0 {
            return Err(ProviderStoreError::NotFound);
        }
        Ok(())
    }

    async fn clear_defaults(&self) -> Result<(), DbErr> {
        let rows = llm_providers::Entity::find()
            .filter(llm_providers::Column::IsDefault.eq(true))
            .all(&self.db)
            .await?;
        for row in rows {
            let mut active: llm_providers::ActiveModel = row.into();
            active.is_default = Set(false);
            active.update(&self.db).await?;
        }
        Ok(())
    }

    /// Build the active provider: the DB default row if present, else the
    /// `ANTHROPIC_API_KEY` env fallback, else `None`. Best-effort — a row that
    /// fails to build a client is logged and falls through to the env key.
    pub async fn resolve(&self) -> Option<Arc<dyn LlmProvider>> {
        match self.default_row().await {
            Ok(Some(row)) => match AnthropicProvider::with_model(row.api_key, row.model) {
                Ok(provider) => return Some(Arc::new(provider) as Arc<dyn LlmProvider>),
                Err(e) => tracing::warn!(error = %e, name = %row.name,
                    "could not build configured LLM provider; trying env fallback"),
            },
            Ok(None) => {}
            Err(e) => {
                tracing::warn!(error = %e, "could not read llm_providers; trying env fallback")
            }
        }
        provider_from_env()
    }

    async fn default_row(&self) -> Result<Option<llm_providers::Model>, DbErr> {
        llm_providers::Entity::find()
            .filter(llm_providers::Column::IsDefault.eq(true))
            .order_by_asc(llm_providers::Column::Id)
            .one(&self.db)
            .await
    }
}

/// The `ANTHROPIC_API_KEY` env fallback (the pre-#20 behaviour). A missing or
/// blank key, or a construction failure, leaves the LLM paths disabled.
fn provider_from_env() -> Option<Arc<dyn LlmProvider>> {
    let key = std::env::var("ANTHROPIC_API_KEY")
        .ok()
        .filter(|k| !k.trim().is_empty())?;
    match AnthropicProvider::new(key) {
        Ok(provider) => Some(Arc::new(provider) as Arc<dyn LlmProvider>),
        Err(e) => {
            tracing::warn!(error = %e, "could not build LLM provider from env; LLM paths disabled");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::config::{Backend, DbConfig};

    async fn mem_db() -> DatabaseConnection {
        crate::db::connect(&DbConfig {
            backend: Backend::Sqlite {
                path: ":memory:".to_string(),
            },
        })
        .await
        .expect("connect in-memory db")
    }

    #[tokio::test]
    async fn upsert_requires_admin_and_lists_without_keys() {
        let svc = ProviderService::new(mem_db().await);
        let plain = CurrentUser {
            id: "alice".to_string(),
            is_admin: false,
        };
        let input = || ProviderInput {
            name: "anthropic".to_string(),
            model: "claude-sonnet-4-6".to_string(),
            api_key: "secret".to_string(),
            is_default: true,
        };
        assert!(matches!(
            svc.upsert(&plain, input()).await,
            Err(ProviderStoreError::Forbidden)
        ));

        let admin = CurrentUser::local_admin();
        let info = svc.upsert(&admin, input()).await.expect("upsert");
        assert_eq!(info.name, "anthropic");
        assert!(info.is_default);

        // The serialized list never carries the key.
        let listed = svc.list().await.expect("list");
        assert_eq!(listed.len(), 1);
        let json = serde_json::to_string(&listed).unwrap();
        assert!(!json.contains("secret"), "api key leaked into list output");
    }

    #[tokio::test]
    async fn upsert_default_is_unique() {
        let svc = ProviderService::new(mem_db().await);
        let admin = CurrentUser::local_admin();
        for name in ["one", "two"] {
            svc.upsert(
                &admin,
                ProviderInput {
                    name: name.to_string(),
                    model: "m".to_string(),
                    api_key: "k".to_string(),
                    is_default: true,
                },
            )
            .await
            .expect("upsert");
        }
        let defaults = svc.list().await.unwrap();
        let count = defaults.iter().filter(|p| p.is_default).count();
        assert_eq!(count, 1, "only the latest default should remain set");
    }
}
