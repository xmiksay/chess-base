//! The concrete MCP tools registered into the [`ToolRegistry`]. The dispatch
//! plumbing (auth, JSON-RPC framing, the envelope) lives in [`super`]; this
//! module is just the tool builders + handlers. Every handler receives the
//! resolved [`CurrentUser`] so its reads/writes scope to the caller (ADR-0016).

use std::collections::BTreeMap;

use serde_json::{json, Value};

use super::{Tool, ToolOutcome, ToolRegistry};
use crate::engine::{DEFAULT_DEPTH, MAX_DEPTH, MAX_MOVETIME_MS};
use crate::server::state::AppState;

/// The default registry. An `echo` stub proves dispatch; the engine facade
/// registers `engine_analyse`; the study tools (#17) mutate the caller's studies;
/// the pre-chewed DB tools (#28) answer position/reference queries; the
/// interactive `analyse_position` tool (#33) bundles engine + DB + features into
/// one grounded snapshot; the study-preprocessing tools (ADR-0027) expose the
/// engine/DB tree + concept stages as plain data (no internal LLM — the model
/// that annotates them is the MCP client, not a tool).
pub fn default_registry() -> ToolRegistry {
    let mut registry = ToolRegistry::new();
    registry.register(echo_tool());
    registry.register(engine_analyse_tool());
    super::study_tools::register(&mut registry);
    super::db_tools::register(&mut registry);
    super::analysis::register(&mut registry);
    super::preprocess::register(&mut registry);
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
            "opening_tree",
            "danger_map",
            "position_concepts",
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
