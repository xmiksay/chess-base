//! MCP collection-import tools (issue #183): ingest a PGN upload or trigger a
//! Lichess/Chess.com sync into a database. Thin wrappers over
//! [`ImportService`], mirroring `imports/routes.rs`.

use serde_json::{json, Value};

use super::db_tools::json_outcome;
use super::{Tool, ToolOutcome, ToolRegistry};
use crate::imports::{ImportError, ImportService, ImportSource, ImportSummary};
use crate::server::identity::CurrentUser;
use crate::server::state::AppState;

/// Register the import tools into `registry`.
pub fn register(registry: &mut ToolRegistry) {
    registry.register(import_pgn_tool());
    registry.register(import_sync_tool());
}

/// Ingest a (possibly multi-game) PGN upload into a database the caller may
/// write.
fn import_pgn_tool() -> Tool {
    Tool::new(
        "import_pgn",
        "Ingest a PGN (one or many games) into a database you may write. The \
         response carries `game_ids` — the new games' ids, in PGN order — for \
         chaining into `db_read_game`, `analyse_game` or `game_save_as_study`. \
         A game already present (same permalink or identical content) is \
         dropped and counted in `duplicates`, so `imported: 0, duplicates: 1` \
         means the game was already there. A bad game inside a multi-game \
         upload is skipped, not fatal, with a reason per skip in `errors`. \
         Discover a `database_id` via `list_databases`.",
        json!({
            "type": "object",
            "properties": {
                "database_id": { "type": "integer", "description": "Database to ingest into." },
                "pgn": { "type": "string", "description": "PGN text (one or many games)." }
            },
            "required": ["database_id", "pgn"]
        }),
        |app, user, args| async move { import_pgn(app, user, args).await },
    )
}

async fn import_pgn(app: AppState, user: CurrentUser, args: Value) -> ToolOutcome {
    let Some(database_id) = args.get("database_id").and_then(Value::as_i64) else {
        return ToolOutcome::error("Invalid arguments: missing integer field `database_id`.");
    };
    let Some(pgn) = args.get("pgn").and_then(Value::as_str) else {
        return ToolOutcome::error("Invalid arguments: missing string field `pgn`.");
    };
    let service = ImportService::new(app.db.clone());
    match service.import_pgn(&user, database_id as i32, pgn).await {
        Ok(summary) => json_outcome(&summary_view(&summary)),
        Err(e) => import_error(e),
    }
}

/// Trigger a Lichess / Chess.com sync into a database the caller may write.
fn import_sync_tool() -> Tool {
    Tool::new(
        "import_sync",
        "Sync games from Lichess or Chess.com into a database you may write. \
         Resumes from the cursor persisted per (database, source), so a re-sync \
         only fetches new games. `token` is an optional personal access token \
         (Lichess; raises rate limits) — leave it out for a public sync.",
        json!({
            "type": "object",
            "properties": {
                "database_id": { "type": "integer", "description": "Database to sync into." },
                "source": { "type": "string", "enum": ["lichess", "chesscom"], "description": "Provider to pull from." },
                "username": { "type": "string", "description": "Provider username to sync." },
                "token": { "type": "string", "description": "Optional personal access token (Lichess)." }
            },
            "required": ["database_id", "source", "username"]
        }),
        |app, user, args| async move { import_sync(app, user, args).await },
    )
}

async fn import_sync(app: AppState, user: CurrentUser, args: Value) -> ToolOutcome {
    let Some(database_id) = args.get("database_id").and_then(Value::as_i64) else {
        return ToolOutcome::error("Invalid arguments: missing integer field `database_id`.");
    };
    let Some(source_str) = args.get("source").and_then(Value::as_str) else {
        return ToolOutcome::error("Invalid arguments: missing string field `source`.");
    };
    let Some(source) = ImportSource::parse(source_str) else {
        return ToolOutcome::error(format!(
            "Invalid arguments: unknown `source` '{source_str}' (expected lichess or chesscom)."
        ));
    };
    let Some(username) = args.get("username").and_then(Value::as_str) else {
        return ToolOutcome::error("Invalid arguments: missing string field `username`.");
    };
    let token = args.get("token").and_then(Value::as_str);

    let service = ImportService::new(app.db.clone());
    match service
        .sync(&user, database_id as i32, source, username, token)
        .await
    {
        Ok(summary) => json_outcome(&summary_view(&summary)),
        Err(e) => import_error(e),
    }
}

/// Wire shape shared by both import tools:
/// `{ imported, skipped, duplicates, game_ids[], errors[] }`.
fn summary_view(summary: &ImportSummary) -> Value {
    json!({
        "imported": summary.imported,
        "skipped": summary.skipped,
        "duplicates": summary.duplicates,
        "game_ids": summary.game_ids,
        "errors": summary.errors,
    })
}

/// Map an [`ImportError`] to a tool outcome without leaking DB internals.
fn import_error(error: ImportError) -> ToolOutcome {
    match error {
        ImportError::Db(_) => ToolOutcome::error("import failed: database error"),
        other => ToolOutcome::error(other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn registry() -> ToolRegistry {
        let mut registry = ToolRegistry::new();
        register(&mut registry);
        registry
    }

    #[test]
    fn registers_the_import_tools() {
        let list = registry().list();
        let tools = list["tools"].as_array().expect("tools array");
        for expected in ["import_pgn", "import_sync"] {
            assert!(
                tools.iter().any(|t| t["name"] == expected),
                "missing tool {expected}"
            );
        }
    }

    #[tokio::test]
    async fn import_pgn_returns_game_ids_and_counts_duplicates() {
        use sea_orm::{ActiveModelTrait, Set};

        let app = dummy_app().await;
        let db = crate::db::entities::databases::ActiveModel {
            owner_id: Set(None),
            name: Set("Games".to_string()),
            kind: Set("own".to_string()),
            ..Default::default()
        }
        .insert(&app.db)
        .await
        .expect("create database");

        const GAME: &str = "[Event \"Casual\"]\n[White \"A\"]\n[Black \"B\"]\n[Result \"1-0\"]\n\n1. e4 e5 2. Bc4 Nc6 3. Qh5 Nf6 4. Qxf7# 1-0\n";
        let args = json!({ "database_id": db.id, "pgn": GAME });

        // First call creates the game and hands back its id for chaining.
        let first = import_pgn(app.clone(), CurrentUser::local_admin(), args.clone()).await;
        assert!(!first.is_error, "unexpected error: {}", first.text);
        let body: Value = serde_json::from_str(&first.text).expect("json body");
        assert_eq!(body["imported"], 1);
        assert_eq!(body["duplicates"], 0);
        assert!(body["game_ids"][0].as_i64().expect("new game id") > 0);

        // Re-importing the identical PGN is a reported duplicate, not a silent
        // 0/0 "success".
        let second = import_pgn(app, CurrentUser::local_admin(), args).await;
        assert!(!second.is_error);
        let body: Value = serde_json::from_str(&second.text).expect("json body");
        assert_eq!(body["imported"], 0);
        assert_eq!(body["duplicates"], 1);
        assert!(body["game_ids"].as_array().expect("array").is_empty());
    }

    #[tokio::test]
    async fn sync_rejects_an_unknown_source() {
        let outcome = import_sync(
            dummy_app().await,
            CurrentUser::local_admin(),
            json!({ "database_id": 1, "source": "carlsen-com", "username": "magnus" }),
        )
        .await;
        assert!(outcome.is_error);
        assert!(outcome.text.contains("unknown `source`"));
    }

    async fn dummy_app() -> AppState {
        let db = crate::db::connect(&crate::db::config::DbConfig {
            backend: crate::db::config::Backend::Sqlite {
                path: ":memory:".to_string(),
            },
        })
        .await
        .expect("connect in-memory db");
        AppState {
            db,
            mode: crate::server::config::Mode::Local,
            engine_service: None,
            llm_provider: None,
        }
    }
}
