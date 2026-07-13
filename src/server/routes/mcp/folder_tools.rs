//! MCP folder tools (issue #183, #164/ADR-0030): the study-organizing folder
//! tree, entirely absent from MCP until now. Thin wrappers over
//! [`FolderService`], mirroring `folders/routes.rs`.

use serde_json::{json, Value};

use super::db_tools::json_outcome;
use super::{Tool, ToolOutcome, ToolRegistry};
use crate::folders::{FolderError, FolderService};
use crate::server::identity::CurrentUser;
use crate::server::state::AppState;

/// Register the folder tools into `registry`.
pub fn register(registry: &mut ToolRegistry) {
    registry.register(folder_list_tool());
    registry.register(folder_create_tool());
    registry.register(folder_update_tool());
    registry.register(folder_delete_tool());
}

/// All folders visible to the caller (own ∪ global).
fn folder_list_tool() -> Tool {
    Tool::new(
        "folder_list",
        "List the folders you can see — your own plus the global ones — each \
         with its id, parent id and name. Assemble the tree from each row's \
         `parent_id` (`null` = root). Use this to discover the `folder_id` \
         `study_set_folder` and the study-creating tools need.",
        json!({ "type": "object", "properties": {} }),
        |app, user, _args| async move { folder_list(app, user).await },
    )
}

async fn folder_list(app: AppState, user: CurrentUser) -> ToolOutcome {
    let service = FolderService::new(app.db.clone());
    match service.list(&user).await {
        Ok(rows) => json_outcome(&views(rows)),
        Err(e) => folder_error(e),
    }
}

/// Create a folder, optionally inside a parent.
fn folder_create_tool() -> Tool {
    Tool::new(
        "folder_create",
        "Create a folder to organize studies into. Set `parent_id` to nest it \
         under an existing folder you may write (must be owned the same way — no \
         mixing an own folder under a global one or vice versa). Set `global: \
         true` to create an admin-managed folder visible to everyone (requires \
         admin). Returns the new folder id.",
        json!({
            "type": "object",
            "properties": {
                "name": { "type": "string", "description": "Folder name." },
                "parent_id": { "type": "integer", "description": "Parent folder id (optional; omit for the root)." },
                "global": { "type": "boolean", "description": "Make it a global (admin) folder." }
            },
            "required": ["name"]
        }),
        |app, user, args| async move { folder_create(app, user, args).await },
    )
}

async fn folder_create(app: AppState, user: CurrentUser, args: Value) -> ToolOutcome {
    let Some(name) = args.get("name").and_then(Value::as_str) else {
        return ToolOutcome::error("Invalid arguments: missing string field `name`.");
    };
    let parent_id = args
        .get("parent_id")
        .and_then(Value::as_i64)
        .map(|n| n as i32);
    let global = args.get("global").and_then(Value::as_bool).unwrap_or(false);

    let service = FolderService::new(app.db.clone());
    match service.create(&user, parent_id, name, global).await {
        Ok(folder) => ToolOutcome::ok(json!({ "id": folder.id }).to_string()),
        Err(e) => folder_error(e),
    }
}

/// Rename and/or move a folder the caller may write.
fn folder_update_tool() -> Tool {
    Tool::new(
        "folder_update",
        "Rename and/or move a folder you may write. Set `name` to rename it. Set \
         `reparent: true` to move it — `parent_id` gives the new parent, or omit \
         `parent_id` (with `reparent: true`) to move it to the root. Moving into \
         itself or one of its own descendants is rejected as a cycle.",
        json!({
            "type": "object",
            "properties": {
                "folder_id": { "type": "integer", "description": "Folder to update." },
                "name": { "type": "string", "description": "New name (optional)." },
                "reparent": {
                    "type": "boolean",
                    "description": "Move the folder; see `parent_id` (default false)."
                },
                "parent_id": {
                    "type": "integer",
                    "description": "New parent folder id when `reparent` is set (omit to move to the root)."
                }
            },
            "required": ["folder_id"]
        }),
        |app, user, args| async move { folder_update(app, user, args).await },
    )
}

async fn folder_update(app: AppState, user: CurrentUser, args: Value) -> ToolOutcome {
    let Some(folder_id) = args.get("folder_id").and_then(Value::as_i64) else {
        return ToolOutcome::error("Invalid arguments: missing integer field `folder_id`.");
    };
    let name = args.get("name").and_then(Value::as_str);
    let reparent = args
        .get("reparent")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let parent_id = args
        .get("parent_id")
        .and_then(Value::as_i64)
        .map(|n| n as i32);

    let service = FolderService::new(app.db.clone());
    let mut updated = None;
    if let Some(name) = name {
        updated = Some(match service.rename(&user, folder_id as i32, name).await {
            Ok(folder) => folder,
            Err(e) => return folder_error(e),
        });
    }
    if reparent {
        updated = Some(
            match service.reparent(&user, folder_id as i32, parent_id).await {
                Ok(folder) => folder,
                Err(e) => return folder_error(e),
            },
        );
    }
    match updated {
        Some(folder) => ToolOutcome::ok(json!({ "id": folder.id }).to_string()),
        None => ToolOutcome::error(
            "Invalid arguments: set `name` and/or `reparent` to change something.",
        ),
    }
}

/// Delete a folder the caller may write, cascading child folders and unfiling
/// contained studies.
fn folder_delete_tool() -> Tool {
    Tool::new(
        "folder_delete",
        "Delete a folder you may write. Its whole subtree of child folders is \
         removed with it; every study anywhere in that subtree is unfiled \
         (moved to the root), never deleted.",
        json!({
            "type": "object",
            "properties": {
                "folder_id": { "type": "integer", "description": "Folder to delete." }
            },
            "required": ["folder_id"]
        }),
        |app, user, args| async move { folder_delete(app, user, args).await },
    )
}

async fn folder_delete(app: AppState, user: CurrentUser, args: Value) -> ToolOutcome {
    let Some(folder_id) = args.get("folder_id").and_then(Value::as_i64) else {
        return ToolOutcome::error("Invalid arguments: missing integer field `folder_id`.");
    };
    let service = FolderService::new(app.db.clone());
    match service.delete(&user, folder_id as i32).await {
        Ok(()) => ToolOutcome::ok("ok"),
        Err(e) => folder_error(e),
    }
}

fn views(rows: Vec<crate::db::entities::folders::Model>) -> Vec<Value> {
    rows.into_iter()
        .map(|f| {
            json!({
                "id": f.id,
                "owner_id": f.owner_id,
                "parent_id": f.parent_id,
                "name": f.name,
                "global": f.owner_id.is_none(),
            })
        })
        .collect()
}

/// Map a [`FolderError`] to a tool outcome without leaking DB internals.
fn folder_error(error: FolderError) -> ToolOutcome {
    match error {
        FolderError::Db(_) => ToolOutcome::error("folder operation failed: database error"),
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
    fn registers_the_folder_tools() {
        let list = registry().list();
        let tools = list["tools"].as_array().expect("tools array");
        for expected in [
            "folder_list",
            "folder_create",
            "folder_update",
            "folder_delete",
        ] {
            assert!(
                tools.iter().any(|t| t["name"] == expected),
                "missing tool {expected}"
            );
        }
    }

    #[tokio::test]
    async fn update_without_name_or_reparent_is_a_no_op_error() {
        let outcome = folder_update(
            dummy_app().await,
            CurrentUser::local_admin(),
            json!({ "folder_id": 1 }),
        )
        .await;
        assert!(outcome.is_error);
        assert!(outcome.text.contains("set `name` and/or `reparent`"));
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
