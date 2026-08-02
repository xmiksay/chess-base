//! Admin-managed LLM provider registry (issue #20), backing the `llm_providers`
//! table. The old startup resolution (default row → Anthropic client, env
//! fallback) was removed with the hand-rolled assistant (#198); the embedded
//! entanglement agent engine re-wires provider resolution in a later step.
//!
//! API keys are **server-side only**: [`ProviderService::list`] returns
//! [`ProviderInfo`] without the key — keys never reach the SPA.

use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, DbErr, EntityTrait, QueryFilter, QueryOrder,
    Set,
};

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

/// CRUD over `llm_providers`.
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
