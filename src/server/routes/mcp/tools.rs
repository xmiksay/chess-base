//! The concrete MCP tools registered into the [`ToolRegistry`]. The dispatch
//! plumbing (auth, JSON-RPC framing, the envelope) lives in [`super`]; this
//! module is just the tool builders + handlers. Every handler receives the
//! resolved [`CurrentUser`] so its reads/writes scope to the caller (ADR-0016).

use std::collections::BTreeMap;

use serde_json::{json, Value};

use super::{Tool, ToolOutcome, ToolRegistry};
use crate::engine::{Limits, MAX_DEPTH, MAX_MOVETIME_MS};
use crate::position::STARTPOS_FEN;
use crate::search::report::PositionReportService;
use crate::server::identity::CurrentUser;
use crate::server::state::AppState;
use crate::studies::{StudyError, StudyService};
use crate::study_gen::tree::TreeConfig;
use crate::study_gen::{generate_study_live, GenerateError, GenerateParams};

/// Per-position engine search depth for `generate_study` when unspecified; capped
/// server-side via [`Limits::clamped`].
const DEFAULT_GENERATE_DEPTH: u32 = 18;

/// The default registry. An `echo` stub proves dispatch; the engine facade
/// registers `engine_analyse`; the study tools (#17) mutate the caller's studies;
/// the pre-chewed DB tools (#28) answer position/reference queries; the
/// interactive `analyse_position` tool (#33) bundles engine + DB + features into
/// one grounded snapshot.
pub fn default_registry() -> ToolRegistry {
    let mut registry = ToolRegistry::new();
    registry.register(echo_tool());
    registry.register(engine_analyse_tool());
    registry.register(study_create_tool());
    registry.register(study_add_move_tool());
    registry.register(study_annotate_tool());
    registry.register(generate_study_tool());
    super::db_tools::register(&mut registry);
    super::analysis::register(&mut registry);
    registry
}

/// Stub tool: echoes its `text` argument back. Proves the full
/// initialize → tools/list → tools/call path without any Epic 9 dependency.
fn echo_tool() -> Tool {
    Tool::new(
        "echo",
        "Echo the provided text back. A connectivity/diagnostic stub; \
         the real engine and database tools are registered by Epic 9.",
        json!({
            "type": "object",
            "properties": {
                "text": { "type": "string", "description": "Text to echo back." }
            },
            "required": ["text"]
        }),
        |_app, _user, args| async move {
            match args.get("text").and_then(Value::as_str) {
                Some(text) => ToolOutcome::ok(text.to_string()),
                None => ToolOutcome::error("Invalid arguments: missing string field `text`"),
            }
        },
    )
}

/// Interactive analysis tool. The MCP facade over the pooled [`EngineService`]:
/// it routes through the **same** `analyse` the batch pipeline calls in-process,
/// so one engine pool backs both paths.
///
/// [`EngineService`]: crate::engine::EngineService
fn engine_analyse_tool() -> Tool {
    Tool::new(
        "engine_analyse",
        "Analyse a chess position with the configured UCI engine (Stockfish/Lc0). \
         Returns the evaluation (centipawns or mate), the principal variation and \
         the best move — use it as ground truth when annotating positions. \
         Requires the server to be started with an engine configured.",
        json!({
            "type": "object",
            "properties": {
                "fen": { "type": "string", "description": "Position to analyse, in FEN." },
                "depth": {
                    "type": "integer", "minimum": 1, "maximum": MAX_DEPTH,
                    "description": "Search depth in plies. Defaults to a fixed depth if omitted; values are capped server-side."
                },
                "movetime_ms": {
                    "type": "integer", "minimum": 1, "maximum": MAX_MOVETIME_MS,
                    "description": "Search time budget in milliseconds (optional); capped server-side."
                }
            },
            "required": ["fen"]
        }),
        |app, _user, args| async move { engine_analyse(app, args).await },
    )
}

/// Run the `engine_analyse` tool: validate args, call the pooled service, and
/// return the [`Analysis`] as pretty JSON. Errors (no engine, bad FEN, engine
/// failure) come back as `isError` outcomes, never panics.
///
/// [`Analysis`]: crate::engine::Analysis
async fn engine_analyse(app: AppState, args: Value) -> ToolOutcome {
    let service = match &app.engine_service {
        Some(service) => service.clone(),
        None => {
            return ToolOutcome::error(
                "No engine configured: start chess-base with --engine / CHESS_BASE_ENGINE.",
            )
        }
    };

    let fen = match super::db_tools::fen_arg(&args) {
        Some(fen) => fen,
        None => return ToolOutcome::error("Invalid arguments: missing string field `fen`."),
    };

    let limits = match super::db_tools::limits_arg(&args) {
        Ok(limits) => limits,
        Err(msg) => return ToolOutcome::error(msg),
    };

    match service.analyse(&fen, &limits, &BTreeMap::new()).await {
        Ok(analysis) => match serde_json::to_string_pretty(&analysis) {
            Ok(text) => ToolOutcome::ok(text),
            Err(e) => ToolOutcome::error(format!("failed to serialise analysis: {e}")),
        },
        Err(e) => ToolOutcome::error(format!("engine analysis failed: {e}")),
    }
}

// --- Study tools: all scoped to the caller (ADR 0007/0011/0016) ----------

/// Create an empty study owned by the caller (or a global one, admin-only).
fn study_create_tool() -> Tool {
    Tool::new(
        "study_create",
        "Create a new (empty) study in a database. The study is owned by you; set \
         `global: true` to create an admin-managed study visible to everyone \
         (requires admin). Returns the new study id.",
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

/// Append a legal SAN move to a node of one of the caller's studies.
fn study_add_move_tool() -> Tool {
    Tool::new(
        "study_add_move",
        "Append a move (SAN) as a child of a node in one of your studies. The move \
         is validated against the legal moves in that position. You may only edit \
         your own studies. Returns the new node id.",
        json!({
            "type": "object",
            "properties": {
                "study_id": { "type": "integer", "description": "Study to edit." },
                "from_node_id": {
                    "type": "integer", "minimum": 0,
                    "description": "Parent node id (0 is the root)."
                },
                "san": { "type": "string", "description": "Move in SAN, e.g. `Nf3`." }
            },
            "required": ["study_id", "from_node_id", "san"]
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
    let Some(san) = args.get("san").and_then(Value::as_str) else {
        return ToolOutcome::error("Invalid arguments: missing string field `san`.");
    };

    let service = StudyService::new(app.db.clone());
    match service
        .add_move(&user, study_id as i32, from_node_id as usize, san)
        .await
    {
        Ok(node_id) => ToolOutcome::ok(json!({ "node_id": node_id }).to_string()),
        Err(e) => study_error(e),
    }
}

/// Attach a comment and/or NAG to a node of one of the caller's studies.
fn study_annotate_tool() -> Tool {
    Tool::new(
        "study_annotate",
        "Attach a comment and/or a NAG (numeric annotation glyph) to a node in one \
         of your studies. You may only edit your own studies.",
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

/// Generate a fully annotated study from a start position — the AI-assisted
/// study-generation orchestrator (#115). Ties the Epic 9 stages together end to
/// end (tree builder → batch LLM annotation + verification → persist).
fn generate_study_tool() -> Tool {
    Tool::new(
        "generate_study",
        "Generate an annotated study for a position: build a variation tree from \
         the master/reference games, annotate it with a language model, verify \
         every concrete claim against the engine and database (ground truth), and \
         save the result as a study you own. Requires both an engine and a \
         language model configured. Returns the new study id and a summary.",
        json!({
            "type": "object",
            "properties": {
                "database_id": { "type": "integer", "description": "Database the new study belongs to." },
                "name": { "type": "string", "description": "Name for the new study." },
                "fen": { "type": "string", "description": "Start position in FEN; defaults to the standard opening." },
                "global": { "type": "boolean", "description": "Make it a global (admin) study (requires admin)." },
                "model": { "type": "string", "description": "Language model id; defaults to the provider's default." },
                "engine_depth": {
                    "type": "integer", "minimum": 1, "maximum": MAX_DEPTH,
                    "description": "Per-position engine search depth in plies; capped server-side."
                },
                "tree": {
                    "type": "object",
                    "description": "Optional tree pruning thresholds (max_depth, max_children, max_nodes, min_frequency, eval_margin_cp)."
                }
            },
            "required": ["database_id", "name"]
        }),
        |app, user, args| async move { generate_study(app, user, args).await },
    )
}

async fn generate_study(app: AppState, user: CurrentUser, args: Value) -> ToolOutcome {
    let Some(database_id) = args.get("database_id").and_then(Value::as_i64) else {
        return ToolOutcome::error("Invalid arguments: missing integer field `database_id`.");
    };
    let Some(name) = args.get("name").and_then(Value::as_str) else {
        return ToolOutcome::error("Invalid arguments: missing string field `name`.");
    };

    let engine = match &app.engine_service {
        Some(engine) => engine.clone(),
        None => {
            return ToolOutcome::error(
                "No engine configured: start chess-base with --engine / CHESS_BASE_ENGINE.",
            )
        }
    };
    let provider =
        match &app.llm_provider {
            Some(provider) => provider.clone(),
            None => return ToolOutcome::error(
                "No language model configured: set ANTHROPIC_API_KEY to enable study generation.",
            ),
        };

    let tree = match args.get("tree") {
        None | Some(Value::Null) => TreeConfig::default(),
        Some(value) => match serde_json::from_value(value.clone()) {
            Ok(config) => config,
            Err(e) => return ToolOutcome::error(format!("Invalid arguments: bad `tree`: {e}")),
        },
    };
    let params = GenerateParams {
        database_id: database_id as i32,
        name: name.to_string(),
        global: args.get("global").and_then(Value::as_bool).unwrap_or(false),
        start_fen: args
            .get("fen")
            .and_then(Value::as_str)
            .filter(|fen| !fen.trim().is_empty())
            .unwrap_or(STARTPOS_FEN)
            .to_string(),
        tree,
        model: args
            .get("model")
            .and_then(Value::as_str)
            .map(str::to_string),
    };
    let depth = args
        .get("engine_depth")
        .and_then(Value::as_u64)
        .map(|d| d as u32)
        .unwrap_or(DEFAULT_GENERATE_DEPTH);
    let limits = Limits::depth(depth).clamped();
    let reports = PositionReportService::new(app.db.clone());
    let studies = StudyService::new(app.db.clone());

    match generate_study_live(
        &engine,
        &reports,
        provider.as_ref(),
        &studies,
        &user,
        &params,
        limits,
    )
    .await
    {
        Ok(outcome) => ToolOutcome::ok(
            json!({
                "id": outcome.study.id,
                "name": outcome.study.name,
                "node_count": outcome.node_count,
                "rejected": outcome.rejected.len(),
            })
            .to_string(),
        ),
        Err(e) => generate_error(e),
    }
}

/// Map a [`GenerateError`] onto a tool `isError` outcome with a client-safe
/// message — never leaks a raw DB error, engine output or provider detail.
fn generate_error(error: GenerateError) -> ToolOutcome {
    ToolOutcome::error(error.client_message())
}

/// Map a [`StudyError`] to a tool `isError` outcome — never leaks a raw DB error.
fn study_error(error: StudyError) -> ToolOutcome {
    match error {
        StudyError::Db(_) => ToolOutcome::error("study operation failed: database error"),
        other => ToolOutcome::error(other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_lists_the_stub_engine_and_study_tools() {
        let list = default_registry().list();
        let tools = list["tools"].as_array().expect("tools array");
        for expected in [
            "echo",
            "engine_analyse",
            "study_create",
            "study_add_move",
            "study_annotate",
            "generate_study",
        ] {
            assert!(
                tools.iter().any(|t| t["name"] == expected),
                "missing tool {expected}"
            );
        }
        assert_eq!(list["tools"][0]["inputSchema"]["type"], "object");
    }

    #[test]
    fn engine_tool_requires_fen() {
        let list = default_registry().list();
        let tools = list["tools"].as_array().unwrap();
        let engine = tools
            .iter()
            .find(|t| t["name"] == "engine_analyse")
            .expect("engine_analyse tool");
        assert_eq!(engine["inputSchema"]["required"][0], "fen");
    }

    #[test]
    fn db_errors_are_not_leaked_in_tool_text() {
        let outcome = study_error(StudyError::NotFound);
        assert!(outcome.is_error);
        assert_eq!(outcome.text, "study not found");
    }
}
