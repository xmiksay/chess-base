//! Transport-agnostic user-settings service: the one place per-user UI
//! preferences (theme, board theme, default database) live, so the HTTP routes
//! and the planned MCP tools are thin callers (mirrors [`crate::databases`] and
//! [`crate::engine::EngineRegistry`]).
//!
//! Persistence reuses the key/value `settings` table — no new entity. Each user's
//! preferences are stored as one JSON blob under a per-user key
//! (`user_settings:{id}`), so local mode (single implicit admin) and server mode
//! (many users) share the same storage with no schema change. Engine *paths* are
//! operator config owned by [`EngineRegistry`]; this service is purely the
//! per-user view layer on top.
//!
//! [`EngineRegistry`]: crate::engine::EngineRegistry

use sea_orm::sea_query::OnConflict;
use sea_orm::{DatabaseConnection, DbErr, EntityTrait, QueryFilter, QuerySelect, Set};
use serde::{Deserialize, Serialize};

use crate::db::entities::{databases, settings};
use crate::server::identity::{scope, CurrentUser};

/// Prefix for the per-user settings key in the `settings` table.
const USER_SETTINGS_PREFIX: &str = "user_settings:";

/// Allowed color-scheme values for [`UserSettings::theme`].
const THEMES: [&str; 3] = ["light", "dark", "system"];

/// A user's persisted UI preferences. Every field is optional so the SPA can
/// fall back to its own defaults, and so adding a field stays backward-compatible
/// with already-stored blobs (`#[serde(default)]` tolerates missing keys).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserSettings {
    /// Color scheme: `light` | `dark` | `system`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub theme: Option<String>,
    /// Board color theme (frontend-defined, e.g. `brown` / `blue` / `green`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub board_theme: Option<String>,
    /// Piece set (frontend-defined, e.g. `cburnett`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub piece_set: Option<String>,
    /// Default database id for new searches/imports. Must be visible to the user.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_database_id: Option<i32>,
}

/// Why a settings operation failed. Transport-agnostic — the HTTP / MCP layer
/// maps each variant onto its own status / error envelope.
#[derive(Debug, thiserror::Error)]
pub enum SettingsError {
    /// A field held a value outside its allowed set (e.g. an unknown theme, or a
    /// `default_database_id` not visible to the caller).
    #[error("{0}")]
    InvalidInput(String),
    /// The stored settings JSON could not be (de)serialized — corrupt blob.
    #[error("corrupt settings: {0}")]
    Serde(#[from] serde_json::Error),
    /// Underlying database error (never surfaced verbatim to clients).
    #[error(transparent)]
    Db(#[from] DbErr),
}

/// Per-user settings over the `settings` table. Holds a connection handle (cheap
/// to clone — SeaORM wraps an `Arc`'d pool).
#[derive(Clone)]
pub struct SettingsService {
    db: DatabaseConnection,
}

impl SettingsService {
    pub fn new(db: DatabaseConnection) -> Self {
        Self { db }
    }

    /// The caller's stored settings, or all-default if they have saved none yet.
    pub async fn get(&self, user: &CurrentUser) -> Result<UserSettings, SettingsError> {
        match self.read(&key_for(user)).await? {
            Some(json) => Ok(serde_json::from_str(&json)?),
            None => Ok(UserSettings::default()),
        }
    }

    /// Replace the caller's settings wholesale after validation. Returns the
    /// stored value so the caller sees the canonical (trimmed) result.
    pub async fn set(
        &self,
        user: &CurrentUser,
        mut settings: UserSettings,
    ) -> Result<UserSettings, SettingsError> {
        settings.normalize();
        self.validate(user, &settings).await?;
        self.write(&key_for(user), serde_json::to_string(&settings)?)
            .await?;
        Ok(settings)
    }

    /// Enforce the value constraints: a known theme, and a `default_database_id`
    /// the caller can actually see (own ∪ global). Frontend-defined fields
    /// (board theme / piece set) are free-form strings, validated only as
    /// non-empty by [`UserSettings::normalize`].
    async fn validate(
        &self,
        user: &CurrentUser,
        settings: &UserSettings,
    ) -> Result<(), SettingsError> {
        if let Some(theme) = &settings.theme {
            if !THEMES.contains(&theme.as_str()) {
                return Err(SettingsError::InvalidInput(format!(
                    "invalid theme '{theme}' (expected one of {})",
                    THEMES.join(", ")
                )));
            }
        }
        if let Some(id) = settings.default_database_id {
            let visible = databases::Entity::find_by_id(id)
                .filter(scope(databases::Column::OwnerId, user))
                .select_only()
                .column(databases::Column::Id)
                .into_tuple::<i32>()
                .one(&self.db)
                .await?
                .is_some();
            if !visible {
                return Err(SettingsError::InvalidInput(format!(
                    "database {id} is not available"
                )));
            }
        }
        Ok(())
    }

    async fn read(&self, key: &str) -> Result<Option<String>, DbErr> {
        Ok(settings::Entity::find_by_id(key.to_string())
            .one(&self.db)
            .await?
            .map(|m| m.value))
    }

    async fn write(&self, key: &str, value: String) -> Result<(), DbErr> {
        let model = settings::ActiveModel {
            key: Set(key.to_string()),
            value: Set(value),
        };
        settings::Entity::insert(model)
            .on_conflict(
                OnConflict::column(settings::Column::Key)
                    .update_column(settings::Column::Value)
                    .to_owned(),
            )
            .exec(&self.db)
            .await?;
        Ok(())
    }
}

impl UserSettings {
    /// Trim string fields, dropping any that become empty so blanks never persist.
    fn normalize(&mut self) {
        trim_to_none(&mut self.theme);
        trim_to_none(&mut self.board_theme);
        trim_to_none(&mut self.piece_set);
    }
}

/// Trim an optional string in place, collapsing an empty result to `None`.
fn trim_to_none(field: &mut Option<String>) {
    if let Some(s) = field {
        let trimmed = s.trim();
        if trimmed.is_empty() {
            *field = None;
        } else if trimmed.len() != s.len() {
            *field = Some(trimmed.to_string());
        }
    }
}

/// The `settings` key holding this user's preference blob.
fn key_for(user: &CurrentUser) -> String {
    format!("{USER_SETTINGS_PREFIX}{}", user.id)
}

pub mod routes;

#[cfg(test)]
mod tests;
