//! Transport-agnostic engine registry (ADR 0005 amendment).
//!
//! [`EngineRegistry`] persists several [`EngineConfig`]s plus a `default_engine`
//! selector in the key/value `settings` store — no new DB entity. It is the one
//! place engine selection lives, so the analysis WebSocket and the planned MCP
//! tools are thin callers (mirroring the `StudyService` split).
//!
//! Reads (`list` / `get` / `default_name` / [`resolve_default`]) are open to any
//! authenticated caller — every user needs an engine to analyse. Writes
//! (`upsert` / `remove` / `set_default`) are operator config and require admin
//! (the implicit admin in local mode; an admin account in server mode).
//!
//! [`resolve_default`]: EngineRegistry::resolve_default

use sea_orm::sea_query::OnConflict;
use sea_orm::{DatabaseConnection, DbErr, EntityTrait, Set};

use crate::db::entities::settings;
use crate::server::identity::{assert_admin, CurrentUser};

use super::EngineConfig;

/// `settings` key holding the JSON array of registered engines.
const ENGINES_KEY: &str = "engines";
/// `settings` key holding the name of the selected default engine.
const DEFAULT_KEY: &str = "default_engine";

/// Why a registry operation failed. Transport-agnostic — the HTTP / MCP layer
/// maps each variant onto its own status / error envelope.
#[derive(Debug, thiserror::Error)]
pub enum RegistryError {
    /// No engine with that name is registered.
    #[error("engine '{0}' not found")]
    NotFound(String),
    /// Authenticated but not permitted: a non-admin attempting a write.
    #[error("not permitted")]
    Forbidden,
    /// The stored `engines` JSON could not be (de)serialized — corrupt settings.
    #[error("corrupt engine settings: {0}")]
    Serde(#[from] serde_json::Error),
    /// Underlying database error (never surfaced verbatim to clients).
    #[error(transparent)]
    Db(#[from] DbErr),
}

/// Persisted multi-engine registry over the `settings` table. Holds a connection
/// handle (cheap to clone — SeaORM wraps an `Arc`'d pool).
#[derive(Clone)]
pub struct EngineRegistry {
    db: DatabaseConnection,
}

impl EngineRegistry {
    pub fn new(db: DatabaseConnection) -> Self {
        Self { db }
    }

    /// Every registered engine, in insertion order.
    pub async fn list(&self) -> Result<Vec<EngineConfig>, RegistryError> {
        match self.read(ENGINES_KEY).await? {
            Some(json) => Ok(serde_json::from_str(&json)?),
            None => Ok(Vec::new()),
        }
    }

    /// The engine registered under `name`, if any.
    pub async fn get(&self, name: &str) -> Result<Option<EngineConfig>, RegistryError> {
        Ok(self.list().await?.into_iter().find(|e| e.name == name))
    }

    /// The selected default engine's name, if one has been chosen.
    pub async fn default_name(&self) -> Result<Option<String>, RegistryError> {
        Ok(self.read(DEFAULT_KEY).await?)
    }

    /// Resolve the effective engine to run, applying the resolution order
    /// (first wins): a user-configured registry default → the embedded
    /// `bundled-stockfish` build → an auto-downloaded binary (#11).
    pub async fn resolve_default(&self) -> Result<Option<EngineConfig>, RegistryError> {
        Ok(resolve(
            self.default_entry().await?,
            bundled_engine(),
            downloaded_engine(),
        ))
    }

    /// Add or replace an engine (keyed by `name`). Admin-only.
    pub async fn upsert(
        &self,
        user: &CurrentUser,
        config: EngineConfig,
    ) -> Result<(), RegistryError> {
        assert_admin(user).map_err(|_| RegistryError::Forbidden)?;
        self.upsert_config(config).await
    }

    /// Remove an engine by name. Clears the default selector if it pointed at
    /// the removed engine. Admin-only.
    pub async fn remove(&self, user: &CurrentUser, name: &str) -> Result<(), RegistryError> {
        assert_admin(user).map_err(|_| RegistryError::Forbidden)?;
        let mut engines = self.list().await?;
        let before = engines.len();
        engines.retain(|e| e.name != name);
        if engines.len() == before {
            return Err(RegistryError::NotFound(name.to_string()));
        }
        self.save_list(&engines).await?;
        if self.default_name().await?.as_deref() == Some(name) {
            self.delete(DEFAULT_KEY).await?;
        }
        Ok(())
    }

    /// Point the `default_engine` selector at a registered engine. Admin-only.
    pub async fn set_default(&self, user: &CurrentUser, name: &str) -> Result<(), RegistryError> {
        assert_admin(user).map_err(|_| RegistryError::Forbidden)?;
        if !self.list().await?.iter().any(|e| e.name == name) {
            return Err(RegistryError::NotFound(name.to_string()));
        }
        self.write(DEFAULT_KEY, name.to_string()).await?;
        Ok(())
    }

    /// Seed an operator-supplied engine (the `--engine` CLI flag) into the
    /// persistent registry at startup. Makes it the default only when none has
    /// been chosen yet, so a persisted user selection wins across restarts.
    pub async fn seed_default(&self, config: EngineConfig) -> Result<(), RegistryError> {
        self.upsert_config(config).await
    }

    /// The registry entry named by `default_engine`, if both the selector and a
    /// matching entry exist. The "user-configured" slot of [`resolve`].
    async fn default_entry(&self) -> Result<Option<EngineConfig>, RegistryError> {
        let Some(name) = self.default_name().await? else {
            return Ok(None);
        };
        self.get(&name).await
    }

    /// Shared upsert without the admin gate, used by `upsert` and `seed_default`.
    async fn upsert_config(&self, config: EngineConfig) -> Result<(), RegistryError> {
        let name = config.name.clone();
        let mut engines = self.list().await?;
        match engines.iter_mut().find(|e| e.name == name) {
            Some(slot) => *slot = config,
            None => engines.push(config),
        }
        self.save_list(&engines).await?;
        // The first engine added becomes the default so analysis just works.
        if self.default_name().await?.is_none() {
            self.write(DEFAULT_KEY, name).await?;
        }
        Ok(())
    }

    async fn save_list(&self, engines: &[EngineConfig]) -> Result<(), RegistryError> {
        self.write(ENGINES_KEY, serde_json::to_string(engines)?)
            .await?;
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

    async fn delete(&self, key: &str) -> Result<(), DbErr> {
        settings::Entity::delete_by_id(key.to_string())
            .exec(&self.db)
            .await?;
        Ok(())
    }
}

/// Pick the engine to run from the candidates in priority order (first `Some`
/// wins): a user-configured engine, then the embedded `bundled-stockfish`
/// build, then an auto-downloaded binary. Pure, so the order is unit-testable.
pub fn resolve(
    user: Option<EngineConfig>,
    bundled: Option<EngineConfig>,
    downloaded: Option<EngineConfig>,
) -> Option<EngineConfig> {
    user.or(bundled).or(downloaded)
}

/// The embedded `bundled-stockfish` engine when that opt-in build feature is on.
/// A seam for the bundled-engine work tracked alongside this issue; until it
/// lands there is no embedded binary, so this is always `None`.
fn bundled_engine() -> Option<EngineConfig> {
    None
}

/// An engine the auto-download manager (#11) has fetched and registered. A seam
/// until that manager lands.
fn downloaded_engine() -> Option<EngineConfig> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{connect, DbConfig};

    fn admin() -> CurrentUser {
        CurrentUser::local_admin()
    }

    fn plain() -> CurrentUser {
        CurrentUser {
            id: "bob".into(),
            is_admin: false,
        }
    }

    async fn registry() -> EngineRegistry {
        let db = connect(&DbConfig::in_memory()).await.unwrap();
        EngineRegistry::new(db)
    }

    #[test]
    fn resolution_prefers_user_then_bundled_then_downloaded() {
        let user = EngineConfig::new("user", "/u");
        let bundled = EngineConfig::new("bundled", "/b");
        let downloaded = EngineConfig::new("downloaded", "/d");

        // User-configured wins over everything.
        let r = resolve(
            Some(user.clone()),
            Some(bundled.clone()),
            Some(downloaded.clone()),
        );
        assert_eq!(r.unwrap().name, "user");

        // Without a user engine, the bundled build is next.
        let r = resolve(None, Some(bundled), Some(downloaded.clone()));
        assert_eq!(r.unwrap().name, "bundled");

        // Otherwise fall back to an auto-downloaded binary.
        let r = resolve(None, None, Some(downloaded));
        assert_eq!(r.unwrap().name, "downloaded");

        // Nothing available ⇒ no engine.
        assert!(resolve(None, None, None).is_none());
    }

    #[tokio::test]
    async fn upsert_lists_and_gets_engines() {
        let reg = registry().await;
        assert!(reg.list().await.unwrap().is_empty());

        reg.upsert(&admin(), EngineConfig::new("Stockfish", "/sf"))
            .await
            .unwrap();
        reg.upsert(
            &admin(),
            EngineConfig::new("Maia", "/lc0").with_weights("/maia.pb"),
        )
        .await
        .unwrap();

        let all = reg.list().await.unwrap();
        assert_eq!(all.len(), 2);
        assert_eq!(
            reg.get("Maia").await.unwrap().unwrap().weights.unwrap(),
            std::path::PathBuf::from("/maia.pb")
        );
        assert!(reg.get("nope").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn upsert_replaces_an_existing_engine_by_name() {
        let reg = registry().await;
        reg.upsert(&admin(), EngineConfig::new("Stockfish", "/old"))
            .await
            .unwrap();
        reg.upsert(
            &admin(),
            EngineConfig::new("Stockfish", "/new").with_runner("/usr/bin/wine"),
        )
        .await
        .unwrap();

        let all = reg.list().await.unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].path, std::path::PathBuf::from("/new"));
        assert_eq!(
            all[0].runner,
            Some(std::path::PathBuf::from("/usr/bin/wine"))
        );
    }

    #[tokio::test]
    async fn first_engine_becomes_the_default_and_drives_resolution() {
        let reg = registry().await;
        assert!(reg.resolve_default().await.unwrap().is_none());

        reg.upsert(&admin(), EngineConfig::new("Stockfish", "/sf"))
            .await
            .unwrap();
        assert_eq!(
            reg.default_name().await.unwrap().as_deref(),
            Some("Stockfish")
        );
        assert_eq!(
            reg.resolve_default().await.unwrap().unwrap().name,
            "Stockfish"
        );

        // Adding a second engine does not steal the default.
        reg.upsert(&admin(), EngineConfig::new("Maia", "/lc0"))
            .await
            .unwrap();
        assert_eq!(
            reg.resolve_default().await.unwrap().unwrap().name,
            "Stockfish"
        );

        reg.set_default(&admin(), "Maia").await.unwrap();
        assert_eq!(reg.resolve_default().await.unwrap().unwrap().name, "Maia");
    }

    #[tokio::test]
    async fn set_default_rejects_unknown_engine() {
        let reg = registry().await;
        let err = reg.set_default(&admin(), "ghost").await.unwrap_err();
        assert!(matches!(err, RegistryError::NotFound(_)));
    }

    #[tokio::test]
    async fn remove_drops_the_engine_and_clears_a_stale_default() {
        let reg = registry().await;
        reg.upsert(&admin(), EngineConfig::new("Stockfish", "/sf"))
            .await
            .unwrap();
        reg.remove(&admin(), "Stockfish").await.unwrap();

        assert!(reg.list().await.unwrap().is_empty());
        assert!(reg.default_name().await.unwrap().is_none());
        assert!(matches!(
            reg.remove(&admin(), "Stockfish").await.unwrap_err(),
            RegistryError::NotFound(_)
        ));
    }

    #[tokio::test]
    async fn writes_require_admin() {
        let reg = registry().await;
        let cfg = EngineConfig::new("Stockfish", "/sf");
        assert!(matches!(
            reg.upsert(&plain(), cfg.clone()).await.unwrap_err(),
            RegistryError::Forbidden
        ));
        // Seed one as admin so the other writes have a target.
        reg.upsert(&admin(), cfg).await.unwrap();
        assert!(matches!(
            reg.set_default(&plain(), "Stockfish").await.unwrap_err(),
            RegistryError::Forbidden
        ));
        assert!(matches!(
            reg.remove(&plain(), "Stockfish").await.unwrap_err(),
            RegistryError::Forbidden
        ));
    }

    #[tokio::test]
    async fn seed_default_persists_but_does_not_clobber_a_chosen_default() {
        let reg = registry().await;
        reg.upsert(&admin(), EngineConfig::new("A", "/a"))
            .await
            .unwrap();
        reg.upsert(&admin(), EngineConfig::new("B", "/b"))
            .await
            .unwrap();
        reg.set_default(&admin(), "B").await.unwrap();

        // A startup seed upserts its config but leaves the chosen default intact.
        reg.seed_default(EngineConfig::new("C", "/c"))
            .await
            .unwrap();
        assert_eq!(reg.list().await.unwrap().len(), 3);
        assert_eq!(reg.default_name().await.unwrap().as_deref(), Some("B"));
    }
}
