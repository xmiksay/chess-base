//! Ownership-aware LLM provider registry (issue #20, per-user since #198)
//! backing the `llm_providers` table. A row with `owner_id` NULL is a global
//! (admin-managed) provider every user sees; otherwise it belongs to that
//! user. The [`AgentProviderStore`](crate::ai::agent::AgentProviderStore)
//! composes both into each user's resolution catalog.
//!
//! API keys are **server-side only**: [`ProviderService::list`] returns
//! [`ProviderInfo`] with a `has_key` flag — keys never reach the SPA.

use entanglement_provider::Catalog;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, DbErr, EntityTrait, QueryFilter, QueryOrder,
    Set,
};

use crate::db::entities::llm_providers;
use crate::server::identity::{scope, AuthError, CurrentUser};

/// A provider config without its secret key — the only shape exposed to clients.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ProviderInfo {
    pub id: i32,
    pub name: String,
    pub wire: String,
    pub model: String,
    pub base_url: Option<String>,
    /// Whether a key is stored — the key itself is never serialized.
    pub has_key: bool,
    pub is_default: bool,
    pub is_global: bool,
    /// Models selectable on this row (issue #214): the row's own model first,
    /// then the builtin catalog's donations for a same-named provider (the same
    /// donation rule the agent store's `entry_from_row` applies), deduped.
    pub models: Vec<String>,
}

impl From<llm_providers::Model> for ProviderInfo {
    fn from(m: llm_providers::Model) -> Self {
        Self {
            id: m.id,
            has_key: !m.api_key.is_empty(),
            is_default: m.is_default,
            is_global: m.owner_id.is_none(),
            models: vec![m.model.clone()],
            name: m.name,
            wire: m.wire,
            model: m.model,
            base_url: m.base_url,
        }
    }
}

impl ProviderInfo {
    /// Extend `models` with the builtin catalog's entries for a same-named
    /// provider, keeping the row's own model first and deduping.
    fn with_catalog_models(mut self, builtin: &Catalog) -> Self {
        if let Some(provider) = builtin.provider(&self.name) {
            for m in &provider.models {
                if !self.models.contains(&m.id) {
                    self.models.push(m.id.clone());
                }
            }
        }
        self
    }
}

/// Fields to create or update a provider.
pub struct ProviderInput {
    pub name: String,
    pub wire: String,
    pub model: String,
    pub base_url: Option<String>,
    /// `None` ⇒ keep the existing key (or store none on create).
    pub api_key: Option<String>,
    pub is_default: bool,
    /// Write a global (`owner_id` NULL) row — admin only.
    pub is_global: bool,
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

/// CRUD over `llm_providers`, scoped to the calling user.
#[derive(Clone)]
pub struct ProviderService {
    db: DatabaseConnection,
}

impl ProviderService {
    pub fn new(db: DatabaseConnection) -> Self {
        Self { db }
    }

    /// The caller's own providers plus the global ones (no keys), newest first.
    pub async fn list(&self, user: &CurrentUser) -> Result<Vec<ProviderInfo>, ProviderStoreError> {
        let rows = llm_providers::Entity::find()
            .filter(scope(llm_providers::Column::OwnerId, user))
            .order_by_desc(llm_providers::Column::Id)
            .all(&self.db)
            .await?;
        let builtin = Catalog::builtin();
        Ok(rows
            .into_iter()
            .map(|r| ProviderInfo::from(r).with_catalog_models(&builtin))
            .collect())
    }

    /// Create or update a provider by name within its ownership scope. A plain
    /// user writes only their own rows; `is_global` requires admin. Making a
    /// row the default clears the flag on that owner's other rows (the global
    /// default only competes among global rows).
    pub async fn upsert(
        &self,
        user: &CurrentUser,
        input: ProviderInput,
    ) -> Result<ProviderInfo, ProviderStoreError> {
        if input.is_global && !user.is_admin {
            return Err(ProviderStoreError::Forbidden);
        }
        let owner: Option<String> = (!input.is_global).then(|| user.id.clone());

        if input.is_default {
            self.clear_defaults(owner.as_deref()).await?;
        }
        // Find-by-(owner, name) enforces global-name uniqueness in service
        // code: SQL NULLs are distinct in the unique index, so two global rows
        // with one name would otherwise both insert.
        let existing = llm_providers::Entity::find()
            .filter(owner_condition(owner.as_deref()))
            .filter(llm_providers::Column::Name.eq(input.name.clone()))
            .one(&self.db)
            .await?;
        let model = match existing {
            Some(row) => {
                let mut active: llm_providers::ActiveModel = row.into();
                active.wire = Set(input.wire);
                active.model = Set(input.model);
                active.base_url = Set(input.base_url);
                if let Some(key) = input.api_key {
                    active.api_key = Set(key);
                }
                active.is_default = Set(input.is_default);
                active.update(&self.db).await?
            }
            None => {
                llm_providers::ActiveModel {
                    name: Set(input.name),
                    wire: Set(input.wire),
                    model: Set(input.model),
                    base_url: Set(input.base_url),
                    api_key: Set(input.api_key.unwrap_or_default()),
                    is_default: Set(input.is_default),
                    owner_id: Set(owner),
                    ..Default::default()
                }
                .insert(&self.db)
                .await?
            }
        };
        Ok(ProviderInfo::from(model))
    }

    /// Delete a provider by id: a user deletes their own rows, an admin also
    /// the global ones.
    pub async fn delete(&self, user: &CurrentUser, id: i32) -> Result<(), ProviderStoreError> {
        let row = llm_providers::Entity::find_by_id(id)
            .one(&self.db)
            .await?
            .ok_or(ProviderStoreError::NotFound)?;
        crate::server::identity::assert_can_write(row.owner_id.as_deref(), user)?;
        llm_providers::Entity::delete_by_id(id)
            .exec(&self.db)
            .await?;
        Ok(())
    }

    /// The provider/model pair a new session starts on: the user's own default
    /// row, else the global default, else `None`. (The `ANTHROPIC_API_KEY` env
    /// fallback lives in the agent store's house context, not here.)
    pub async fn resolve_default_for(
        &self,
        user: &CurrentUser,
    ) -> Result<Option<(String, String)>, ProviderStoreError> {
        for owner in [Some(user.id.as_str()), None] {
            let row = llm_providers::Entity::find()
                .filter(owner_condition(owner))
                .filter(llm_providers::Column::IsDefault.eq(true))
                .one(&self.db)
                .await?;
            if let Some(row) = row {
                return Ok(Some((row.name, row.model)));
            }
        }
        Ok(None)
    }

    /// Clear `is_default` on every row of one owner scope (`None` = global).
    async fn clear_defaults(&self, owner: Option<&str>) -> Result<(), DbErr> {
        let rows = llm_providers::Entity::find()
            .filter(owner_condition(owner))
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

/// Exact-owner filter (`None` ⇒ the global rows), as opposed to [`scope`]'s
/// own-∪-global read scope.
fn owner_condition(owner: Option<&str>) -> sea_orm::sea_query::SimpleExpr {
    match owner {
        Some(id) => llm_providers::Column::OwnerId.eq(id),
        None => llm_providers::Column::OwnerId.is_null(),
    }
}

#[cfg(test)]
mod tests;
