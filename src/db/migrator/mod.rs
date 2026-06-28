//! SeaORM migrations, one module per migration. Each uses the schema builder so
//! the same migration runs on both SQLite (local) and Postgres (server).

use sea_orm_migration::prelude::*;

mod m0001_init;
mod m0002_core_schema;
mod m0003_auth;
mod m0004_oauth;

pub struct Migrator;

#[async_trait::async_trait]
impl MigratorTrait for Migrator {
    fn migrations() -> Vec<Box<dyn MigrationTrait>> {
        vec![
            Box::new(m0001_init::Migration),
            Box::new(m0002_core_schema::Migration),
            Box::new(m0003_auth::Migration),
            Box::new(m0004_oauth::Migration),
        ]
    }
}
