//! Database backend selection — the local/server split lives here.

/// Which storage backend the app runs against.
#[derive(Debug, Clone)]
pub enum Backend {
    /// Embedded SQLite (local mode). `:memory:` is supported for tests.
    Sqlite { path: String },
    /// PostgreSQL (server mode), given as a full connection URL.
    Postgres { url: String },
}

/// Resolved database configuration.
#[derive(Debug, Clone)]
pub struct DbConfig {
    pub backend: Backend,
}

impl DbConfig {
    pub fn sqlite(path: impl Into<String>) -> Self {
        Self {
            backend: Backend::Sqlite { path: path.into() },
        }
    }

    pub fn postgres(url: impl Into<String>) -> Self {
        Self {
            backend: Backend::Postgres { url: url.into() },
        }
    }

    /// In-memory SQLite, used by tests.
    pub fn in_memory() -> Self {
        Self::sqlite(":memory:")
    }

    pub fn is_memory(&self) -> bool {
        matches!(&self.backend, Backend::Sqlite { path } if path == ":memory:")
    }

    /// The sqlx/SeaORM connection URL for this config.
    pub fn url(&self) -> String {
        match &self.backend {
            Backend::Sqlite { path } if path == ":memory:" => "sqlite::memory:".to_string(),
            // `mode=rwc` creates the file if it does not exist.
            Backend::Sqlite { path } => format!("sqlite://{path}?mode=rwc"),
            Backend::Postgres { url } => url.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_sqlite_file_url() {
        assert_eq!(
            DbConfig::sqlite("data/chess.db").url(),
            "sqlite://data/chess.db?mode=rwc"
        );
    }

    #[test]
    fn memory_is_detected() {
        assert!(DbConfig::in_memory().is_memory());
        assert_eq!(DbConfig::in_memory().url(), "sqlite::memory:");
    }
}
