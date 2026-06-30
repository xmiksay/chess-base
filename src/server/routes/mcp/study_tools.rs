//! MCP study tools: the move-tree editing surface, all scoped to the caller
//! (ADR 0007/0011/0016). Each tool is a thin wrapper over [`StudyService`] — the
//! same service the HTTP study routes call — so build/read/import/export all go
//! through one transport-agnostic layer. Dispatch/JSON-RPC framing lives in
//! [`super`]; this module is just the tool builders + handlers.

use serde_json::{json, Value};

use super::db_tools::json_outcome;
use super::{Tool, ToolOutcome, ToolRegistry};
use crate::server::identity::CurrentUser;
use crate::server::state::AppState;
use crate::studies::{MoveInput, StudyError, StudyService};

/// Register the study tools into `registry`.
pub fn register(registry: &mut ToolRegistry) {
    registry.register(study_create_tool());
    registry.register(study_get_tool());
    registry.register(study_import_pgn_tool());
    registry.register(study_add_move_tool());
    registry.register(study_annotate_tool());
    registry.register(study_export_tool());
}

/// Create an empty study owned by the caller (or a global one, admin-only).
fn study_create_tool() -> Tool {
    Tool::new(
        "study_create",
        "Create a new (empty) study in a database. The study is owned by you; set \
         `global: true` to create an admin-managed study visible to everyone \
         (requires admin). Use `list_databases` first to pick a `database_id`. \
         Returns the new study id.",
        json!({
            "type": "object",
            "properties": {
                "database_id": { "type": "integer", "description": "Database the study belongs to." },
                "name": { "type": "string", "description": "Study name." },
                "global": { "type": "boolean", "description": "Make it a global (admin) study." }
            },
            "required": ["database_id", "name"]
        }),
        |app, user, args| async move { study_create(app, user, args).await },
    )
}

async fn study_create(app: AppState, user: CurrentUser, args: Value) -> ToolOutcome {
    let Some(database_id) = args.get("database_id").and_then(Value::as_i64) else {
        return ToolOutcome::error("Invalid arguments: missing integer field `database_id`.");
    };
    let Some(name) = args.get("name").and_then(Value::as_str) else {
        return ToolOutcome::error("Invalid arguments: missing string field `name`.");
    };
    let global = args.get("global").and_then(Value::as_bool).unwrap_or(false);

    let service = StudyService::new(app.db.clone());
    match service
        .create(&user, database_id as i32, name, global)
        .await
    {
        Ok(study) => ToolOutcome::ok(json!({ "id": study.id }).to_string()),
        Err(e) => study_error(e),
    }
}

/// Read back a study's metadata and full move tree (with node ids).
fn study_get_tool() -> Tool {
    Tool::new(
        "study_get",
        "Read a study you can see: its metadata plus the full move tree with every \
         node id, SAN, comment, NAG glyphs and `[%eval]`. Use it to discover the \
         `node_id`s needed by `study_annotate` when editing a study you did not just \
         build. Scoped to your studies and the global ones.",
        json!({
            "type": "object",
            "properties": {
                "study_id": { "type": "integer", "description": "Study to read." }
            },
            "required": ["study_id"]
        }),
        |app, user, args| async move { study_get(app, user, args).await },
    )
}

async fn study_get(app: AppState, user: CurrentUser, args: Value) -> ToolOutcome {
    let Some(study_id) = args.get("study_id").and_then(Value::as_i64) else {
        return ToolOutcome::error("Invalid arguments: missing integer field `study_id`.");
    };
    let service = StudyService::new(app.db.clone());
    let study = match service.get(&user, study_id as i32).await {
        Ok(study) => study,
        Err(e) => return study_error(e),
    };
    // The stored tree is our own JSON; parse it to embed the node tree inline.
    let Ok(tree) = serde_json::from_str::<Value>(&study.tree_json) else {
        return ToolOutcome::error("study read failed: corrupt study tree");
    };
    json_outcome(&json!({
        "id": study.id,
        "database_id": study.database_id,
        "owner_id": study.owner_id,
        "name": study.name,
        "global": study.owner_id.is_none(),
        "tree": tree,
    }))
}

/// Import a PGN game as a new study in one call (vs. dozens of `study_add_move`s).
fn study_import_pgn_tool() -> Tool {
    Tool::new(
        "study_import_pgn",
        "Import a PGN game as a new study in a single call: the first game's \
         mainline, variations, comments and NAGs are parsed into the move tree \
         (every move validated for legality). Pairs with `study_export` for a full \
         export → edit → re-import round trip. The study is owned by you; set \
         `global: true` for an admin study (requires admin). Returns the new study \
         id.",
        json!({
            "type": "object",
            "properties": {
                "database_id": { "type": "integer", "description": "Database the study belongs to." },
                "name": { "type": "string", "description": "Study name." },
                "pgn": { "type": "string", "description": "PGN movetext (first game) to import." },
                "global": { "type": "boolean", "description": "Make it a global (admin) study." }
            },
            "required": ["database_id", "name", "pgn"]
        }),
        |app, user, args| async move { study_import_pgn(app, user, args).await },
    )
}

async fn study_import_pgn(app: AppState, user: CurrentUser, args: Value) -> ToolOutcome {
    let Some(database_id) = args.get("database_id").and_then(Value::as_i64) else {
        return ToolOutcome::error("Invalid arguments: missing integer field `database_id`.");
    };
    let Some(name) = args.get("name").and_then(Value::as_str) else {
        return ToolOutcome::error("Invalid arguments: missing string field `name`.");
    };
    let Some(pgn) = args.get("pgn").and_then(Value::as_str) else {
        return ToolOutcome::error("Invalid arguments: missing string field `pgn`.");
    };
    let global = args.get("global").and_then(Value::as_bool).unwrap_or(false);

    let service = StudyService::new(app.db.clone());
    match service
        .import_pgn(&user, database_id as i32, name, pgn, global)
        .await
    {
        Ok(study) => ToolOutcome::ok(json!({ "id": study.id }).to_string()),
        Err(e) => study_error(e),
    }
}

/// Append a move (SAN or UCI) to a node of one of the caller's studies.
fn study_add_move_tool() -> Tool {
    Tool::new(
        "study_add_move",
        "Append a move as a child of a node in one of your studies, given as `san` \
         (e.g. `Nf3`) or `uci` (e.g. `g1f3`). Prefer `uci` to avoid SAN \
         disambiguation pitfalls. The move is validated against the legal moves in \
         that position. Optionally attach a `comment` and/or `nag` in the same call. \
         You may only edit your own studies. Returns the new node id, the FEN it \
         reaches and the canonical SAN stored.",
        json!({
            "type": "object",
            "properties": {
                "study_id": { "type": "integer", "description": "Study to edit." },
                "from_node_id": {
                    "type": "integer", "minimum": 0,
                    "description": "Parent node id (0 is the root)."
                },
                "san": { "type": "string", "description": "Move in SAN, e.g. `Nf3` (or use `uci`)." },
                "uci": { "type": "string", "description": "Move in UCI, e.g. `g1f3` (alternative to `san`)." },
                "comment": { "type": "string", "description": "Comment to attach to the new node (optional)." },
                "nag": {
                    "type": "integer", "minimum": 0, "maximum": 255,
                    "description": "NAG code to attach to the new node, e.g. 1 = good move (optional)."
                }
            },
            "required": ["study_id", "from_node_id"]
        }),
        |app, user, args| async move { study_add_move(app, user, args).await },
    )
}

async fn study_add_move(app: AppState, user: CurrentUser, args: Value) -> ToolOutcome {
    let Some(study_id) = args.get("study_id").and_then(Value::as_i64) else {
        return ToolOutcome::error("Invalid arguments: missing integer field `study_id`.");
    };
    let Some(from_node_id) = args.get("from_node_id").and_then(Value::as_u64) else {
        return ToolOutcome::error("Invalid arguments: missing integer field `from_node_id`.");
    };
    // Exactly one of `san` / `uci`; `san` wins if both are present.
    let mv = match (
        args.get("san").and_then(Value::as_str),
        args.get("uci").and_then(Value::as_str),
    ) {
        (Some(san), _) => MoveInput::San(san.to_string()),
        (None, Some(uci)) => MoveInput::Uci(uci.to_string()),
        (None, None) => {
            return ToolOutcome::error("Invalid arguments: provide a move as `san` or `uci`.")
        }
    };
    let comment = args
        .get("comment")
        .and_then(Value::as_str)
        .map(str::to_string);
    let nag = args.get("nag").and_then(Value::as_u64).map(|n| n as u8);

    let service = StudyService::new(app.db.clone());
    let added = match service
        .add_move_detailed(&user, study_id as i32, from_node_id as usize, mv)
        .await
    {
        Ok(added) => added,
        Err(e) => return study_error(e),
    };
    // Inline comment/NAG saves the caller a second round trip (issue #125).
    if comment.is_some() || nag.is_some() {
        if let Err(e) = service
            .annotate(&user, study_id as i32, added.node_id, comment, nag)
            .await
        {
            return study_error(e);
        }
    }
    json_outcome(&added)
}

/// Attach a comment and/or NAG to a node of one of the caller's studies.
fn study_annotate_tool() -> Tool {
    Tool::new(
        "study_annotate",
        "Attach a comment and/or a NAG (numeric annotation glyph) to a node in one \
         of your studies. Use `study_get` to discover node ids. You may only edit \
         your own studies.",
        json!({
            "type": "object",
            "properties": {
                "study_id": { "type": "integer", "description": "Study to edit." },
                "node_id": { "type": "integer", "minimum": 0, "description": "Node to annotate." },
                "comment": { "type": "string", "description": "Free-text comment (optional)." },
                "nag": {
                    "type": "integer", "minimum": 0, "maximum": 255,
                    "description": "NAG code, e.g. 1 = good move (optional)."
                }
            },
            "required": ["study_id", "node_id"]
        }),
        |app, user, args| async move { study_annotate(app, user, args).await },
    )
}

async fn study_annotate(app: AppState, user: CurrentUser, args: Value) -> ToolOutcome {
    let Some(study_id) = args.get("study_id").and_then(Value::as_i64) else {
        return ToolOutcome::error("Invalid arguments: missing integer field `study_id`.");
    };
    let Some(node_id) = args.get("node_id").and_then(Value::as_u64) else {
        return ToolOutcome::error("Invalid arguments: missing integer field `node_id`.");
    };
    let comment = args
        .get("comment")
        .and_then(Value::as_str)
        .map(str::to_string);
    let nag = args.get("nag").and_then(Value::as_u64).map(|n| n as u8);

    let service = StudyService::new(app.db.clone());
    match service
        .annotate(&user, study_id as i32, node_id as usize, comment, nag)
        .await
    {
        Ok(()) => ToolOutcome::ok("ok"),
        Err(e) => study_error(e),
    }
}

/// Export a study (visible to the caller) as PGN — standard movetext or a
/// Lichess-study chapter — with NAGs, comments and pinned shapes preserved.
fn study_export_tool() -> Tool {
    Tool::new(
        "study_export",
        "Export a study you can see as PGN with NAG glyphs, comments and pinned \
         board shapes (`[%csl]`/`[%cal]`). `format: \"lichess\"` wraps the movetext \
         in PGN header tags for a Lichess-study chapter; the default `pgn` emits \
         headerless movetext. Returns the PGN text — a git-versionable artifact \
         re-importable via `study_import_pgn`.",
        json!({
            "type": "object",
            "properties": {
                "study_id": { "type": "integer", "description": "Study to export." },
                "format": {
                    "type": "string", "enum": ["pgn", "lichess"],
                    "description": "Output format; defaults to `pgn`."
                }
            },
            "required": ["study_id"]
        }),
        |app, user, args| async move { study_export(app, user, args).await },
    )
}

async fn study_export(app: AppState, user: CurrentUser, args: Value) -> ToolOutcome {
    let Some(study_id) = args.get("study_id").and_then(Value::as_i64) else {
        return ToolOutcome::error("Invalid arguments: missing integer field `study_id`.");
    };
    let format = args.get("format").and_then(Value::as_str).unwrap_or("pgn");
    let service = StudyService::new(app.db.clone());
    let exported = match format {
        "pgn" => service.export_pgn(&user, study_id as i32, true).await,
        "lichess" => service.export_lichess(&user, study_id as i32).await,
        other => {
            return ToolOutcome::error(format!(
                "Invalid arguments: unknown `format` '{other}' (use `pgn` or `lichess`)."
            ))
        }
    };
    match exported {
        Ok(pgn) => ToolOutcome::ok(pgn),
        Err(e) => study_error(e),
    }
}

/// Map a [`StudyError`] to a tool `isError` outcome with a client-safe message —
/// never leaks a raw DB error.
pub(super) fn study_error(error: StudyError) -> ToolOutcome {
    match error {
        StudyError::Db(_) => ToolOutcome::error("study operation failed: database error"),
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
    fn registers_the_study_tools() {
        let list = registry().list();
        let tools = list["tools"].as_array().expect("tools array");
        for expected in [
            "study_create",
            "study_get",
            "study_import_pgn",
            "study_add_move",
            "study_annotate",
            "study_export",
        ] {
            assert!(
                tools.iter().any(|t| t["name"] == expected),
                "missing tool {expected}"
            );
        }
    }

    #[test]
    fn add_move_accepts_san_or_uci_but_not_neither() {
        let list = registry().list();
        let tools = list["tools"].as_array().unwrap();
        let add = tools
            .iter()
            .find(|t| t["name"] == "study_add_move")
            .expect("study_add_move tool");
        let required = add["inputSchema"]["required"].as_array().unwrap();
        // Neither `san` nor `uci` is required — exactly one is chosen at call time.
        assert!(required.iter().all(|r| r != "san" && r != "uci"));
        assert!(add["inputSchema"]["properties"]["uci"].is_object());
    }

    #[test]
    fn db_errors_are_not_leaked_in_tool_text() {
        let outcome = study_error(StudyError::NotFound);
        assert!(outcome.is_error);
        assert_eq!(outcome.text, "study not found");
        let outcome = study_error(StudyError::Db(sea_orm::DbErr::Custom("secret".into())));
        assert_eq!(outcome.text, "study operation failed: database error");
    }
}
