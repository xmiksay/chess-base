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
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::Migrator;
    use crate::db::{connect, DbConfig};
    use sea_orm_migration::MigratorTrait;

    /// The folder migration (m0007) must reverse and re-apply cleanly on SQLite —
    /// the one ALTER-per-statement / NULL-distinct-index pitfalls live here.
    #[tokio::test]
    async fn m0007_reverses_and_reapplies() {
        let conn = connect(&DbConfig::in_memory()).await.unwrap();
        // `connect` already migrated up; reverse the latest migration and re-apply.
        Migrator::down(&conn, Some(1)).await.unwrap();
        Migrator::up(&conn, None).await.unwrap();
    }
}
