//! MCP bulk game export (issue #183, #171): bundle several games' stored PGN
//! into one concatenated movetext blob. Mirrors `POST /api/games/export`. Split
//! out of `db_tools.rs` (the pre-chewed *query* surface) since it stays under
//! the project's line cap this way.

use serde_json::Value;

use super::db_tools::game_error;
use super::{Tool, ToolOutcome, ToolRegistry};
use crate::games::GameService;
use crate::server::identity::CurrentUser;
use crate::server::state::AppState;

/// Cap on `db_export_games`' `game_ids`: a huge batch would serialise an
/// unbounded PGN blob.
const MAX_EXPORT_GAMES: usize = 200;

/// Register the bulk export tool into `registry`.
pub fn register(registry: &mut ToolRegistry) {
    registry.register(export_games_tool());
}

/// `db_export_games`: bundle several games' stored PGN into one concatenated
/// movetext blob (issue #171, mirrors `POST /api/games/export`).
fn export_games_tool() -> Tool {
    Tool::new(
        "db_export_games",
        "Bundle several games (by id) into one concatenated PGN movetext blob — \
         the stored games verbatim, no engine involved. Scoped to your databases \
         and the global ones; a game with no stored PGN is skipped. Discover game \
         ids via `db_list_games`.",
        serde_json::json!({
            "type": "object",
            "properties": {
                "game_ids": {
                    "type": "array", "items": { "type": "integer" },
                    "description": format!("Games to export, by id (max {MAX_EXPORT_GAMES})."),
                    "maxItems": MAX_EXPORT_GAMES
                }
            },
            "required": ["game_ids"]
        }),
        |app, user, args| async move { export_games(app, user, args).await },
    )
}

async fn export_games(app: AppState, user: CurrentUser, args: Value) -> ToolOutcome {
    let Some(ids) = args.get("game_ids").and_then(Value::as_array) else {
        return ToolOutcome::error("Invalid arguments: missing array field `game_ids`.");
    };
    if ids.is_empty() {
        return ToolOutcome::error("Invalid arguments: `game_ids` must not be empty.");
    }
    if ids.len() > MAX_EXPORT_GAMES {
        return ToolOutcome::error(format!(
            "Invalid arguments: `game_ids` exceeds the {MAX_EXPORT_GAMES}-game cap."
        ));
    }
    let Some(ids) = ids.iter().map(Value::as_i64).collect::<Option<Vec<_>>>() else {
        return ToolOutcome::error("Invalid arguments: `game_ids` must be integers.");
    };

    let service = GameService::new(app.db.clone());
    let mut parts = Vec::with_capacity(ids.len());
    for id in ids {
        let game = match service.get(&user, id as i32).await {
            Ok(game) => game,
            Err(e) => return game_error(e),
        };
        if let Some(pgn) = game.pgn.as_deref() {
            let trimmed = pgn.trim();
            if !trimmed.is_empty() {
                parts.push(trimmed.to_string());
            }
        }
    }
    if parts.is_empty() {
        return ToolOutcome::error("none of the selected games have a stored PGN");
    }
    ToolOutcome::ok(parts.join("\n\n"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn registry() -> ToolRegistry {
        let mut registry = ToolRegistry::new();
        register(&mut registry);
        registry
    }

    #[test]
    fn registers_the_export_tool() {
        let list = registry().list();
        let tools = list["tools"].as_array().expect("tools array");
        assert!(tools.iter().any(|t| t["name"] == "db_export_games"));
    }

    #[tokio::test]
    async fn rejects_an_empty_game_id_list() {
        let outcome = export_games(
            dummy_app().await,
            CurrentUser::local_admin(),
            json!({ "game_ids": [] }),
        )
        .await;
        assert!(outcome.is_error);
        assert!(outcome.text.contains("must not be empty"));
    }

    #[tokio::test]
    async fn rejects_over_the_cap() {
        let ids: Vec<i64> = (0..(MAX_EXPORT_GAMES as i64 + 1)).collect();
        let outcome = export_games(
            dummy_app().await,
            CurrentUser::local_admin(),
            json!({ "game_ids": ids }),
        )
        .await;
        assert!(outcome.is_error);
        assert!(outcome.text.contains("exceeds"));
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
