//! The concrete MCP tools registered into the [`ToolRegistry`]. The dispatch
//! plumbing (auth, JSON-RPC framing, the envelope) lives in [`super`]; this
//! module is just the tool builders + handlers. Every handler receives the
//! resolved [`CurrentUser`] so its reads/writes scope to the caller (ADR-0016).

use std::collections::BTreeMap;

use serde_json::{json, Value};

use super::{Tool, ToolOutcome, ToolRegistry};
use crate::engine::{Limits, DEFAULT_DEPTH, MAX_DEPTH, MAX_MOVETIME_MS};
use crate::position::STARTPOS_FEN;
use crate::search::report::PositionReportService;
use crate::server::identity::CurrentUser;
use crate::server::state::AppState;
use crate::studies::StudyService;
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
    registry.register(generate_study_tool());
    super::study_tools::register(&mut registry);
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
                    "description": format!(
                        "Search depth in plies. Defaults to depth {DEFAULT_DEPTH} if omitted \
                         (the same default `analyse_position` uses); values are capped server-side."
                    )
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

// --- Study generation orchestrator (scoped to the caller) ----------------

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_lists_the_stub_engine_and_study_tools() {
        let list = default_registry().list();
        let tools = list["tools"].as_array().expect("tools array");
        // The full surface assembled from this module + study_tools + db_tools +
        // analysis — the registry wiring is what this asserts (issue #125).
        for expected in [
            "echo",
            "engine_analyse",
            "generate_study",
            "study_create",
            "study_get",
            "study_import_pgn",
            "study_add_move",
            "study_annotate",
            "study_export",
            "list_databases",
            "db_list_games",
            "db_read_game",
            "analyse_position",
            "analyse_game",
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
}
