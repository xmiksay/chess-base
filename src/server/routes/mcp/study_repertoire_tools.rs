//! MCP **repertoire building** tools (issue #183): fold many games into one
//! study, graft an engine-walked danger tree into an existing study, and fill a
//! study's `[%eval]`s from the engine. Thin wrappers over [`StudyService`],
//! mirroring the `POST /api/studies/merge-games` / `{id}/merge-danger` /
//! `{id}/analyse` HTTP routes; split into their own file so `study_tools.rs`
//! stays under the project's line cap.

use serde_json::{json, Value};

use super::db_tools::json_outcome;
use super::study_tools::study_error;
use super::{Tool, ToolOutcome, ToolRegistry};
use crate::engine::MAX_DEPTH;
use crate::server::identity::CurrentUser;
use crate::server::state::AppState;
use crate::studies::merge_danger::MergeDangerOutcome;
use crate::studies::StudyService;
use crate::study_gen::DangerTree;

/// Per-position engine search depth for `study_analyse` when unspecified;
/// mirrors the HTTP route's default.
const DEFAULT_ANALYSE_DEPTH: u32 = 18;

/// Register the repertoire-building tools into `registry`.
pub fn register(registry: &mut ToolRegistry) {
    registry.register(study_merge_games_tool());
    registry.register(study_merge_danger_tool());
    registry.register(study_analyse_tool());
}

/// Fold many games' mainlines into one repertoire study.
fn study_merge_games_tool() -> Tool {
    Tool::new(
        "study_merge_games",
        "Fold several games' mainlines into one repertoire study: each game's line \
         is deduped in, children are frequency-ordered (the most-played move becomes \
         the mainline), branch points get a `\"N games, X% (labels)\"` stats comment, \
         and transpositions are tagged. Set `study_id` to graft into an existing \
         study you may write; omit it to create a new one (`name` required), \
         optionally filed into `folder_id`. Only standard-start games merge. \
         Discover game ids via `db_list_games`. Returns the study id.",
        json!({
            "type": "object",
            "properties": {
                "game_ids": {
                    "type": "array", "items": { "type": "integer" },
                    "description": "Games to merge, by id."
                },
                "study_id": {
                    "type": "integer",
                    "description": "Graft into this existing study instead of creating one."
                },
                "name": { "type": "string", "description": "Name for the new study (required when `study_id` is omitted)." },
                "folder_id": { "type": "integer", "description": "Folder for the new study (optional)." }
            },
            "required": ["game_ids"]
        }),
        |app, user, args| async move { study_merge_games(app, user, args).await },
    )
}

async fn study_merge_games(app: AppState, user: CurrentUser, args: Value) -> ToolOutcome {
    let Some(game_ids) = args.get("game_ids").and_then(Value::as_array) else {
        return ToolOutcome::error("Invalid arguments: missing array field `game_ids`.");
    };
    let game_ids: Vec<i32> = match game_ids
        .iter()
        .map(|v| v.as_i64())
        .collect::<Option<Vec<_>>>()
    {
        Some(ids) => ids.into_iter().map(|id| id as i32).collect(),
        None => return ToolOutcome::error("Invalid arguments: `game_ids` must be integers."),
    };
    let study_id = args
        .get("study_id")
        .and_then(Value::as_i64)
        .map(|n| n as i32);
    let name = args.get("name").and_then(Value::as_str).map(str::to_string);
    let folder_id = args
        .get("folder_id")
        .and_then(Value::as_i64)
        .map(|n| n as i32);

    let service = StudyService::new(app.db.clone());
    match service
        .merge_games(&user, &game_ids, study_id, name, folder_id)
        .await
    {
        Ok(study) => ToolOutcome::ok(json!({ "id": study.id }).to_string()),
        Err(e) => study_error(e),
    }
}

/// Graft an engine-walked danger tree into an existing study as deduped
/// variations, each annotated from its `DangerTag` (`[%eval]`, a role comment,
/// a `!`/`?!` NAG).
fn study_merge_danger_tool() -> Tool {
    Tool::new(
        "study_merge_danger",
        "Graft an engine-walked danger tree (from the `danger_map` tool) into one \
         of your studies as deduped variations under `at_node_id` (defaults to the \
         root). Every node the graft actually creates is annotated from its role \
         (Weapon/Caution) with `[%eval]`, a short comment quoting the verdict, and \
         a `!`/`?!` NAG — no language model involved. Re-merging the same tree adds \
         nothing (idempotent). You may only edit your own studies.",
        json!({
            "type": "object",
            "properties": {
                "study_id": { "type": "integer", "description": "Study to graft into." },
                "tree": {
                    "type": "object",
                    "description": "The `DangerTree` returned by the `danger_map` tool's `tree` field."
                },
                "at_node_id": {
                    "type": "integer", "minimum": 0,
                    "description": "Graft point node id (defaults to the study's root)."
                }
            },
            "required": ["study_id", "tree"]
        }),
        |app, user, args| async move { study_merge_danger(app, user, args).await },
    )
}

async fn study_merge_danger(app: AppState, user: CurrentUser, args: Value) -> ToolOutcome {
    let Some(study_id) = args.get("study_id").and_then(Value::as_i64) else {
        return ToolOutcome::error("Invalid arguments: missing integer field `study_id`.");
    };
    let Some(tree_value) = args.get("tree") else {
        return ToolOutcome::error("Invalid arguments: missing object field `tree`.");
    };
    let tree: DangerTree = match serde_json::from_value(tree_value.clone()) {
        Ok(tree) => tree,
        Err(e) => return ToolOutcome::error(format!("Invalid arguments: bad `tree`: {e}")),
    };
    let at_node_id = args
        .get("at_node_id")
        .and_then(Value::as_u64)
        .map(|n| n as usize);

    let service = StudyService::new(app.db.clone());
    match service
        .merge_danger(&user, study_id as i32, tree, at_node_id)
        .await
    {
        Ok(outcome) => merge_danger_outcome(&outcome),
        Err(e) => study_error(e),
    }
}

fn merge_danger_outcome(outcome: &MergeDangerOutcome) -> ToolOutcome {
    json_outcome(&json!({
        "study_id": outcome.study.id,
        "added_nodes": outcome.added_nodes,
        "weapons": outcome.weapons,
        "cautions": outcome.cautions,
    }))
}

/// Fill `[%eval]` on every non-terminal node of a study from the engine.
fn study_analyse_tool() -> Tool {
    Tool::new(
        "study_analyse",
        "Walk the engine over every move-bearing node of one of your studies and \
         pin a White-perspective `[%eval]` to each — eval-only, comments/NAGs/shapes \
         are never touched. Useful after `study_import_pgn` or `study_merge_games`, \
         whose trees carry no evals yet. Requires an engine configured. You may \
         only edit your own studies.",
        json!({
            "type": "object",
            "properties": {
                "study_id": { "type": "integer", "description": "Study to analyse." },
                "depth": {
                    "type": "integer", "minimum": 1, "maximum": MAX_DEPTH,
                    "description": format!(
                        "Per-position engine search depth in plies (default {DEFAULT_ANALYSE_DEPTH}); capped server-side."
                    )
                }
            },
            "required": ["study_id"]
        }),
        |app, user, args| async move { study_analyse(app, user, args).await },
    )
}

async fn study_analyse(app: AppState, user: CurrentUser, args: Value) -> ToolOutcome {
    let Some(study_id) = args.get("study_id").and_then(Value::as_i64) else {
        return ToolOutcome::error("Invalid arguments: missing integer field `study_id`.");
    };
    let engine = match &app.engine_service {
        Some(engine) => engine.clone(),
        None => {
            return ToolOutcome::error(
                "No engine configured: start chess-base with --engine / CHESS_BASE_ENGINE.",
            )
        }
    };
    let depth = match super::db_tools::opt_bounded_u64(&args, "depth", MAX_DEPTH as u64) {
        Ok(depth) => depth.map(|d| d as u32).unwrap_or(DEFAULT_ANALYSE_DEPTH),
        Err(msg) => return ToolOutcome::error(msg),
    };

    let service = StudyService::new(app.db.clone());
    match service
        .analyse_study(&engine, &user, study_id as i32, depth)
        .await
    {
        Ok(_) => ToolOutcome::ok("ok"),
        Err(e) => study_error(e),
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
    fn registers_the_repertoire_tools() {
        let list = registry().list();
        let tools = list["tools"].as_array().expect("tools array");
        for expected in ["study_merge_games", "study_merge_danger", "study_analyse"] {
            assert!(
                tools.iter().any(|t| t["name"] == expected),
                "missing tool {expected}"
            );
        }
    }

    #[test]
    fn merge_games_requires_game_ids_only() {
        let list = registry().list();
        let tools = list["tools"].as_array().unwrap();
        let tool = tools
            .iter()
            .find(|t| t["name"] == "study_merge_games")
            .expect("study_merge_games tool");
        assert_eq!(tool["inputSchema"]["required"], json!(["game_ids"]));
    }

    #[tokio::test]
    async fn merge_danger_rejects_malformed_tree() {
        let outcome = study_merge_danger(
            dummy_app().await,
            CurrentUser::local_admin(),
            json!({ "study_id": 1, "tree": "not-a-tree" }),
        )
        .await;
        assert!(outcome.is_error);
        assert!(outcome.text.contains("bad `tree`"));
    }

    #[tokio::test]
    async fn analyse_requires_an_engine() {
        let outcome = study_analyse(
            dummy_app().await,
            CurrentUser::local_admin(),
            json!({ "study_id": 1 }),
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
