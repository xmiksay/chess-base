//! MCP **game document** tools (issue #183): turn a stored game into a study,
//! list a game's linked analyses, read its move tree with variations preserved,
//! and delete a game. Thin wrappers over [`GameService`] / [`StudyService`],
//! mirroring `games/routes.rs`'s `save-as-study` / `studies` / `tree` / delete
//! routes. Split out of `db_tools.rs` (the pre-chewed DB *query* surface) since
//! these mutate or compose more than one service.

use serde_json::{json, Value};

use super::db_tools::{game_error, json_outcome};
use super::study_tools::study_error;
use super::{Tool, ToolOutcome, ToolRegistry};
use crate::engine::MAX_DEPTH;
use crate::games::GameService;
use crate::pgn_tree::pgn::from_pgn_with_start;
use crate::position::STARTPOS_FEN;
use crate::server::identity::CurrentUser;
use crate::server::state::AppState;
use crate::studies::StudyService;

/// Per-position engine search depth for `game_save_as_study`'s `analyse` review,
/// mirroring the HTTP route's default.
const DEFAULT_SAVE_ANALYSE_DEPTH: u32 = 18;

/// Register the game document tools into `registry`.
pub fn register(registry: &mut ToolRegistry) {
    registry.register(game_save_as_study_tool());
    registry.register(game_studies_tool());
    registry.register(game_tree_tool());
    registry.register(game_delete_tool());
}

/// Create an analysis (a study linked via `origin_game_id`) from a game's
/// mainline, optionally engine-reviewed.
fn game_save_as_study_tool() -> Tool {
    Tool::new(
        "game_save_as_study",
        "Create a study (an \"analysis\") from a game's mainline, linked back to \
         it. With `analyse: true` the engine reviews the game first and the study \
         carries `[%eval]` + move-quality NAGs + why-notes (requires an engine \
         configured); otherwise it is a plain linear tree. Always owned by you, \
         optionally filed into `folder_id`. Discover game ids via `db_list_games`. \
         Returns the new study id.",
        json!({
            "type": "object",
            "properties": {
                "game_id": { "type": "integer", "description": "Game to build the study from." },
                "name": { "type": "string", "description": "Name for the new study." },
                "folder_id": { "type": "integer", "description": "Folder for the new study (optional)." },
                "analyse": {
                    "type": "boolean",
                    "description": "Run the engine review and embed evals/NAGs/why-notes (default false)."
                },
                "depth": {
                    "type": "integer", "minimum": 1, "maximum": MAX_DEPTH,
                    "description": format!(
                        "Per-position engine search depth when `analyse` is set (default {DEFAULT_SAVE_ANALYSE_DEPTH}); capped server-side."
                    )
                }
            },
            "required": ["game_id", "name"]
        }),
        |app, user, args| async move { game_save_as_study(app, user, args).await },
    )
}

async fn game_save_as_study(app: AppState, user: CurrentUser, args: Value) -> ToolOutcome {
    let Some(game_id) = args.get("game_id").and_then(Value::as_i64) else {
        return ToolOutcome::error("Invalid arguments: missing integer field `game_id`.");
    };
    let Some(name) = args.get("name").and_then(Value::as_str) else {
        return ToolOutcome::error("Invalid arguments: missing string field `name`.");
    };
    let folder_id = args
        .get("folder_id")
        .and_then(Value::as_i64)
        .map(|n| n as i32);
    let analyse = args
        .get("analyse")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let depth = match super::db_tools::opt_bounded_u64(&args, "depth", MAX_DEPTH as u64) {
        Ok(depth) => depth
            .map(|d| d as u32)
            .unwrap_or(DEFAULT_SAVE_ANALYSE_DEPTH),
        Err(msg) => return ToolOutcome::error(msg),
    };

    let engine = if analyse {
        match &app.engine_service {
            Some(engine) => Some(engine.as_ref()),
            None => {
                return ToolOutcome::error(
                    "No engine configured: start chess-base with --engine / CHESS_BASE_ENGINE.",
                )
            }
        }
    } else {
        None
    };

    let service = StudyService::new(app.db.clone());
    match service
        .create_from_game(
            engine,
            &user,
            game_id as i32,
            name,
            folder_id,
            analyse,
            depth,
        )
        .await
    {
        Ok(study) => ToolOutcome::ok(json!({ "id": study.id }).to_string()),
        Err(e) => study_error(e),
    }
}

/// The studies (analyses) linked to a game.
fn game_studies_tool() -> Tool {
    Tool::new(
        "game_studies",
        "List the studies (analyses) linked to a game via `game_save_as_study`, \
         oldest first. Scoped to your studies and the global ones.",
        json!({
            "type": "object",
            "properties": {
                "game_id": { "type": "integer", "description": "Game to look up." }
            },
            "required": ["game_id"]
        }),
        |app, user, args| async move { game_studies(app, user, args).await },
    )
}

async fn game_studies(app: AppState, user: CurrentUser, args: Value) -> ToolOutcome {
    let Some(game_id) = args.get("game_id").and_then(Value::as_i64) else {
        return ToolOutcome::error("Invalid arguments: missing integer field `game_id`.");
    };
    let service = StudyService::new(app.db.clone());
    match service.studies_for_game(&user, game_id as i32).await {
        Ok(rows) => {
            let views: Vec<Value> = rows
                .into_iter()
                .map(|s| {
                    json!({
                        "id": s.id,
                        "database_id": s.database_id,
                        "owner_id": s.owner_id,
                        "name": s.name,
                        "global": s.owner_id.is_none(),
                        "folder_id": s.folder_id,
                        "origin_game_id": s.origin_game_id,
                    })
                })
                .collect();
            json_outcome(&views)
        }
        Err(e) => study_error(e),
    }
}

/// A game's moves parsed into a [`MoveTree`](crate::pgn_tree::MoveTree),
/// preserving `(…)` sub-variations a flat SAN list would drop.
fn game_tree_tool() -> Tool {
    Tool::new(
        "game_tree",
        "Read a game's moves as a move tree (with `(…)` sub-variations preserved, \
         unlike a flat SAN list). Scoped to your databases and the global ones. \
         Discover game ids via `db_list_games`.",
        json!({
            "type": "object",
            "properties": {
                "game_id": { "type": "integer", "description": "Game to read." }
            },
            "required": ["game_id"]
        }),
        |app, user, args| async move { game_tree(app, user, args).await },
    )
}

async fn game_tree(app: AppState, user: CurrentUser, args: Value) -> ToolOutcome {
    let Some(game_id) = args.get("game_id").and_then(Value::as_i64) else {
        return ToolOutcome::error("Invalid arguments: missing integer field `game_id`.");
    };
    let service = GameService::new(app.db.clone());
    let game = match service.get(&user, game_id as i32).await {
        Ok(game) => game,
        Err(e) => return game_error(e),
    };
    let pgn = game.pgn.as_deref().unwrap_or_default();
    let start_fen = game.start_fen.as_deref().unwrap_or(STARTPOS_FEN);
    match from_pgn_with_start(pgn, start_fen) {
        Ok(tree) => json_outcome(&tree),
        Err(e) => ToolOutcome::error(format!("could not parse game: {e}")),
    }
}

/// Remove a game the caller may write.
fn game_delete_tool() -> Tool {
    Tool::new(
        "game_delete",
        "Delete a game you may write (owner, or admin for a global database). \
         This does not affect studies built from it. Discover game ids via \
         `db_list_games`.",
        json!({
            "type": "object",
            "properties": {
                "game_id": { "type": "integer", "description": "Game to delete." }
            },
            "required": ["game_id"]
        }),
        |app, user, args| async move { game_delete(app, user, args).await },
    )
}

async fn game_delete(app: AppState, user: CurrentUser, args: Value) -> ToolOutcome {
    let Some(game_id) = args.get("game_id").and_then(Value::as_i64) else {
        return ToolOutcome::error("Invalid arguments: missing integer field `game_id`.");
    };
    let service = GameService::new(app.db.clone());
    match service.delete(&user, game_id as i32).await {
        Ok(()) => ToolOutcome::ok("ok"),
        Err(e) => game_error(e),
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
    fn registers_the_game_tools() {
        let list = registry().list();
        let tools = list["tools"].as_array().expect("tools array");
        for expected in [
            "game_save_as_study",
            "game_studies",
            "game_tree",
            "game_delete",
        ] {
            assert!(
                tools.iter().any(|t| t["name"] == expected),
                "missing tool {expected}"
            );
        }
    }

    #[tokio::test]
    async fn save_as_study_requires_an_engine_when_analyse_is_set() {
        let outcome = game_save_as_study(
            dummy_app().await,
            CurrentUser::local_admin(),
            json!({ "game_id": 1, "name": "test", "analyse": true }),
        )
        .await;
        assert!(outcome.is_error);
        assert!(outcome.text.contains("No engine configured"));
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
