//! MCP study **node structure** tools: pin board shapes, reorder/promote a
//! variation, and file a study into a folder — the remaining node/study
//! mutations `study_tools.rs` didn't cover (issue #183). Same thin-wrapper-over
//! [`StudyService`] pattern; split into its own file so `study_tools.rs` stays
//! under the project's line cap.

use serde_json::{json, Value};

use super::study_tools::study_error;
use super::{Tool, ToolOutcome, ToolRegistry};
use crate::pgn_tree::Shape;
use crate::server::identity::CurrentUser;
use crate::server::state::AppState;
use crate::studies::StudyService;

/// Register the study node-structure tools into `registry`.
pub fn register(registry: &mut ToolRegistry) {
    registry.register(study_set_folder_tool());
    registry.register(study_set_shapes_tool());
    registry.register(study_promote_node_tool());
    registry.register(study_reorder_node_tool());
}

/// File a study into a folder (or unfile it) on a study the caller may write.
fn study_set_folder_tool() -> Tool {
    Tool::new(
        "study_set_folder",
        "Move one of your studies into a folder, or omit/null `folder_id` to \
         unfile it (move it to the root). The target folder must be visible and \
         writable to you — use `folder_list` to discover a `folder_id`. You may \
         only edit your own studies.",
        json!({
            "type": "object",
            "properties": {
                "study_id": { "type": "integer", "description": "Study to move." },
                "folder_id": {
                    "type": "integer",
                    "description": "Target folder id; omit or set null to unfile."
                }
            },
            "required": ["study_id"]
        }),
        |app, user, args| async move { study_set_folder(app, user, args).await },
    )
}

async fn study_set_folder(app: AppState, user: CurrentUser, args: Value) -> ToolOutcome {
    let Some(study_id) = args.get("study_id").and_then(Value::as_i64) else {
        return ToolOutcome::error("Invalid arguments: missing integer field `study_id`.");
    };
    let folder_id = args
        .get("folder_id")
        .and_then(Value::as_i64)
        .map(|n| n as i32);

    let service = StudyService::new(app.db.clone());
    match service.set_folder(&user, study_id as i32, folder_id).await {
        Ok(_) => ToolOutcome::ok("ok"),
        Err(e) => study_error(e),
    }
}

/// Pin (or clear, with an empty list) a node's board shapes on a study the
/// caller may write.
fn study_set_shapes_tool() -> Tool {
    Tool::new(
        "study_set_shapes",
        "Replace a node's pinned board shapes (arrows/highlights, `[%cal]`/`[%csl]`) \
         in one of your studies. Pass an empty `shapes` list to clear the pin. Use \
         `study_get` to discover node ids. You may only edit your own studies.",
        json!({
            "type": "object",
            "properties": {
                "study_id": { "type": "integer", "description": "Study to edit." },
                "node_id": { "type": "integer", "minimum": 0, "description": "Node to pin shapes to." },
                "shapes": {
                    "type": "array",
                    "description": "Board shapes to pin; an empty array clears the pin.",
                    "items": { "type": "object" }
                }
            },
            "required": ["study_id", "node_id", "shapes"]
        }),
        |app, user, args| async move { study_set_shapes(app, user, args).await },
    )
}

async fn study_set_shapes(app: AppState, user: CurrentUser, args: Value) -> ToolOutcome {
    let Some(study_id) = args.get("study_id").and_then(Value::as_i64) else {
        return ToolOutcome::error("Invalid arguments: missing integer field `study_id`.");
    };
    let Some(node_id) = args.get("node_id").and_then(Value::as_u64) else {
        return ToolOutcome::error("Invalid arguments: missing integer field `node_id`.");
    };
    let Some(shapes_value) = args.get("shapes") else {
        return ToolOutcome::error("Invalid arguments: missing array field `shapes`.");
    };
    let shapes: Vec<Shape> = match serde_json::from_value(shapes_value.clone()) {
        Ok(shapes) => shapes,
        Err(e) => return ToolOutcome::error(format!("Invalid arguments: bad `shapes`: {e}")),
    };

    let service = StudyService::new(app.db.clone());
    match service
        .set_shapes(&user, study_id as i32, node_id as usize, shapes)
        .await
    {
        Ok(()) => ToolOutcome::ok("ok"),
        Err(e) => study_error(e),
    }
}

/// Promote a variation to the mainline on a study the caller may write.
fn study_promote_node_tool() -> Tool {
    Tool::new(
        "study_promote_node",
        "Promote a variation to the mainline: moves the node to the front of its \
         parent's children in one of your studies. Use `study_get` to discover \
         node ids. You may only edit your own studies.",
        json!({
            "type": "object",
            "properties": {
                "study_id": { "type": "integer", "description": "Study to edit." },
                "node_id": { "type": "integer", "minimum": 0, "description": "Node to promote." }
            },
            "required": ["study_id", "node_id"]
        }),
        |app, user, args| async move { study_promote_node(app, user, args).await },
    )
}

async fn study_promote_node(app: AppState, user: CurrentUser, args: Value) -> ToolOutcome {
    let Some(study_id) = args.get("study_id").and_then(Value::as_i64) else {
        return ToolOutcome::error("Invalid arguments: missing integer field `study_id`.");
    };
    let Some(node_id) = args.get("node_id").and_then(Value::as_u64) else {
        return ToolOutcome::error("Invalid arguments: missing integer field `node_id`.");
    };

    let service = StudyService::new(app.db.clone());
    match service
        .promote_variation(&user, study_id as i32, node_id as usize)
        .await
    {
        Ok(()) => ToolOutcome::ok("ok"),
        Err(e) => study_error(e),
    }
}

/// Reorder a node among its siblings on a study the caller may write.
fn study_reorder_node_tool() -> Tool {
    Tool::new(
        "study_reorder_node",
        "Reorder a node among its siblings, moving it to `index` (0 = mainline) \
         in its parent's child list, in one of your studies. Use `study_get` to \
         discover node ids. You may only edit your own studies.",
        json!({
            "type": "object",
            "properties": {
                "study_id": { "type": "integer", "description": "Study to edit." },
                "node_id": { "type": "integer", "minimum": 0, "description": "Node to reorder." },
                "index": {
                    "type": "integer", "minimum": 0,
                    "description": "Target position among siblings (0 = mainline)."
                }
            },
            "required": ["study_id", "node_id", "index"]
        }),
        |app, user, args| async move { study_reorder_node(app, user, args).await },
    )
}

async fn study_reorder_node(app: AppState, user: CurrentUser, args: Value) -> ToolOutcome {
    let Some(study_id) = args.get("study_id").and_then(Value::as_i64) else {
        return ToolOutcome::error("Invalid arguments: missing integer field `study_id`.");
    };
    let Some(node_id) = args.get("node_id").and_then(Value::as_u64) else {
        return ToolOutcome::error("Invalid arguments: missing integer field `node_id`.");
    };
    let Some(index) = args.get("index").and_then(Value::as_u64) else {
        return ToolOutcome::error("Invalid arguments: missing integer field `index`.");
    };

    let service = StudyService::new(app.db.clone());
    match service
        .reorder_variation(&user, study_id as i32, node_id as usize, index as usize)
        .await
    {
        Ok(()) => ToolOutcome::ok("ok"),
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
    fn registers_the_study_node_tools() {
        let list = registry().list();
        let tools = list["tools"].as_array().expect("tools array");
        for expected in [
            "study_set_folder",
            "study_set_shapes",
            "study_promote_node",
            "study_reorder_node",
        ] {
            assert!(
                tools.iter().any(|t| t["name"] == expected),
                "missing tool {expected}"
            );
        }
    }

    #[test]
    fn set_folder_only_requires_study_id() {
        let list = registry().list();
        let tools = list["tools"].as_array().unwrap();
        let tool = tools
            .iter()
            .find(|t| t["name"] == "study_set_folder")
            .expect("study_set_folder tool");
        assert_eq!(tool["inputSchema"]["required"], json!(["study_id"]));
    }

    #[tokio::test]
    async fn set_shapes_rejects_malformed_shapes() {
        let outcome = study_set_shapes(
            dummy_app().await,
            CurrentUser::local_admin(),
            json!({ "study_id": 1, "node_id": 0, "shapes": "not-an-array" }),
        )
        .await;
        assert!(outcome.is_error);
        assert!(outcome.text.contains("bad `shapes`"));
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
