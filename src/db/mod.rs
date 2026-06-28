//! Database layer: backend-agnostic connection + migrations + entities.

pub mod config;
pub mod entities;
mod migrator;

pub use config::{Backend, DbConfig};

use anyhow::Result;
use sea_orm::{ConnectOptions, Database, DatabaseConnection};
use sea_orm_migration::MigratorTrait;

/// Connect to the configured database and run pending migrations.
pub async fn connect(cfg: &DbConfig) -> Result<DatabaseConnection> {
    let mut opt = ConnectOptions::new(cfg.url());
    // An in-memory SQLite DB lives only as long as its single connection, so
    // the pool must not open more than one.
    if cfg.is_memory() {
        opt.max_connections(1);
    }
    let conn = Database::connect(opt).await?;
    migrator::Migrator::up(&conn, None).await?;
    Ok(conn)
}

#[cfg(test)]
mod tests {
    use super::*;
    use entities::{databases, settings};
    use sea_orm::{ActiveModelTrait, EntityTrait, Set};

    #[tokio::test]
    async fn connects_migrates_and_persists() {
        let conn = connect(&DbConfig::in_memory()).await.unwrap();

        settings::ActiveModel {
            key: Set("theme".to_string()),
            value: Set("dark".to_string()),
        }
        .insert(&conn)
        .await
        .unwrap();

        let got = settings::Entity::find_by_id("theme".to_string())
            .one(&conn)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(got.value, "dark");
    }

    #[tokio::test]
    async fn global_database_has_null_owner() {
        let conn = connect(&DbConfig::in_memory()).await.unwrap();

        let model = databases::ActiveModel {
            owner_id: Set(None),
            name: Set("Master DB".to_string()),
            kind: Set("master".to_string()),
            ..Default::default()
        }
        .insert(&conn)
        .await
        .unwrap();

        assert!(model.owner_id.is_none(), "global database has no owner");
        assert_eq!(model.kind, "master");
    }
}
