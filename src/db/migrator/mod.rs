//! SeaORM migrations, one module per migration. Each uses the schema builder so
//! the same migration runs on both SQLite (local) and Postgres (server).

use sea_orm_migration::prelude::*;

mod m0001_init;
mod m0002_core_schema;
mod m0003_auth;
mod m0004_oauth;
mod m0005_sync_dedup;
mod m0006_assistant;
mod m0007_folders;
mod m0008_agent_engine;

pub struct Migrator;

#[async_trait::async_trait]
impl MigratorTrait for Migrator {
    fn migrations() -> Vec<Box<dyn MigrationTrait>> {
        vec![
            Box::new(m0001_init::Migration),
            Box::new(m0002_core_schema::Migration),
            Box::new(m0003_auth::Migration),
            Box::new(m0004_oauth::Migration),
            Box::new(m0005_sync_dedup::Migration),
            Box::new(m0006_assistant::Migration),
            Box::new(m0007_folders::Migration),
            Box::new(m0008_agent_engine::Migration),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::Migrator;
    use crate::db::entities::{agent_events, agent_grants, agent_sessions, llm_providers};
    use crate::db::{connect, DbConfig};
    use sea_orm::{ActiveModelTrait, ConnectionTrait, Set};
    use sea_orm_migration::MigratorTrait;

    /// The alter-heavy migrations (m0007 folders, m0008 agent engine) must
    /// reverse and re-apply cleanly on SQLite — the ALTER-per-statement /
    /// index-before-column pitfalls live there.
    #[tokio::test]
    async fn latest_migrations_reverse_and_reapply() {
        let conn = connect(&DbConfig::in_memory()).await.unwrap();
        // `connect` already migrated up; reverse m0008 + m0007 and re-apply.
        Migrator::down(&conn, Some(2)).await.unwrap();
        Migrator::up(&conn, None).await.unwrap();
    }

    /// After m0008: the hand-rolled assistant tables are gone, the agent-engine
    /// tables exist and accept rows through their entities, and `llm_providers`
    /// carries the new `owner_id` / `wire` / `base_url` columns (#198).
    #[tokio::test]
    async fn m0008_replaces_assistant_tables_and_extends_llm_providers() {
        let conn = connect(&DbConfig::in_memory()).await.unwrap();

        for table in ["assistant_sessions", "assistant_messages"] {
            let gone = conn
                .execute_unprepared(&format!("SELECT count(*) FROM {table}"))
                .await;
            assert!(gone.is_err(), "{table} should have been dropped");
        }

        let now = chrono::Utc::now().naive_utc();
        agent_grants::ActiveModel {
            user_id: Set("alice".into()),
            tool: Set("study_create".into()),
            arg: Set(None),
            scope: Set("always".into()),
            created_at: Set(now),
            ..Default::default()
        }
        .insert(&conn)
        .await
        .unwrap();
        agent_events::ActiveModel {
            root_session_id: Set("root-1".into()),
            user_id: Set("alice".into()),
            ord: Set(1),
            ts: Set(1_722_600_000),
            session_id: Set("sess-1".into()),
            direction: Set("in".into()),
            payload: Set("{}".into()),
            ..Default::default()
        }
        .insert(&conn)
        .await
        .unwrap();
        agent_sessions::ActiveModel {
            root_session_id: Set("root-1".into()),
            user_id: Set("alice".into()),
            name: Set(Some("first chat".into())),
            agent: Set("study".into()),
            created_at: Set(now),
            last_active: Set(now),
        }
        .insert(&conn)
        .await
        .unwrap();

        let provider = llm_providers::ActiveModel {
            name: Set("mine".into()),
            model: Set("claude-sonnet-4-6".into()),
            api_key: Set("k".into()),
            is_default: Set(false),
            owner_id: Set(Some("alice".into())),
            wire: Set("openai".into()),
            base_url: Set(Some("http://localhost:11434/v1".into())),
            ..Default::default()
        }
        .insert(&conn)
        .await
        .unwrap();
        assert_eq!(provider.owner_id.as_deref(), Some("alice"));
        assert_eq!(provider.wire, "openai");
        assert_eq!(
            provider.base_url.as_deref(),
            Some("http://localhost:11434/v1")
        );
    }
}
