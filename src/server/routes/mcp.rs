//! MCP server: a hand-rolled JSON-RPC 2.0 endpoint at `POST /mcp` (ADR-0008).
//!
//! This module is the **transport + dispatch plumbing only**. It owns a
//! [`ToolRegistry`] that the Epic 9 services (engine facade, DB layer,
//! interactive analysis) plug their tools into — each tool is a name +
//! input-schema + async handler. The handler returns a [`ToolOutcome`] which
//! this layer wraps into the MCP content/`isError` envelope. Mirrors the
//! `site` project's proven pattern; no MCP server crate.

use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::post,
    Json, Router,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::engine::Limits;
use crate::server::state::AppState;

const SERVER_NAME: &str = "chess-base";
const SERVER_VERSION: &str = env!("CARGO_PKG_VERSION");
const PROTOCOL_VERSION: &str = "2025-03-26";

/// Mount the `/mcp` endpoint with the default tool registry.
pub fn router(app: AppState) -> Router {
    let state = McpState {
        app,
        registry: Arc::new(default_registry()),
    };
    Router::new().route("/mcp", post(handle)).with_state(state)
}

/// State threaded into the MCP handler: the app state plus the tool registry.
#[derive(Clone)]
struct McpState {
    app: AppState,
    registry: Arc<ToolRegistry>,
}

// --- Tool registry -------------------------------------------------------

/// The result of running a tool: free text plus the `isError` flag the MCP
/// envelope carries. Tools build these via [`ToolOutcome::ok`] /
/// [`ToolOutcome::error`] and stay ignorant of JSON-RPC framing.
pub struct ToolOutcome {
    pub text: String,
    pub is_error: bool,
}

impl ToolOutcome {
    pub fn ok(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            is_error: false,
        }
    }

    pub fn error(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            is_error: true,
        }
    }
}

/// Boxed async tool handler: `(app state, arguments) -> outcome`.
type ToolFuture = Pin<Box<dyn Future<Output = ToolOutcome> + Send>>;
type ToolFn = Arc<dyn Fn(AppState, Value) -> ToolFuture + Send + Sync>;

/// A registered tool: its `tools/list` metadata plus the dispatch handler.
pub struct Tool {
    pub name: &'static str,
    pub description: &'static str,
    pub input_schema: Value,
    handler: ToolFn,
}

impl Tool {
    /// Build a tool from metadata and an async handler closure. The handler
    /// receives the cloned [`AppState`] and the raw `arguments` object.
    pub fn new<F, Fut>(
        name: &'static str,
        description: &'static str,
        input_schema: Value,
        handler: F,
    ) -> Self
    where
        F: Fn(AppState, Value) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = ToolOutcome> + Send + 'static,
    {
        Self {
            name,
            description,
            input_schema,
            handler: Arc::new(move |state, args| Box::pin(handler(state, args))),
        }
    }
}

/// The set of tools exposed over MCP. Epic 9 issues register their tools here.
#[derive(Default)]
pub struct ToolRegistry {
    tools: Vec<Tool>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&mut self, tool: Tool) {
        self.tools.push(tool);
    }

    fn find(&self, name: &str) -> Option<&Tool> {
        self.tools.iter().find(|t| t.name == name)
    }

    /// The `tools/list` payload: `[{ name, description, inputSchema }]`.
    fn list(&self) -> Value {
        let tools: Vec<Value> = self
            .tools
            .iter()
            .map(|t| {
                json!({
                    "name": t.name,
                    "description": t.description,
                    "inputSchema": t.input_schema,
                })
            })
            .collect();
        json!({ "tools": tools })
    }
}

/// The default registry. An `echo` stub proves dispatch end-to-end; the engine
/// facade (#27) registers `engine_analyse`. Later Epic 9 services append theirs.
fn default_registry() -> ToolRegistry {
    let mut registry = ToolRegistry::new();
    registry.register(echo_tool());
    registry.register(engine_analyse_tool());
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
        |_app, args| async move {
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
                    "type": "integer", "minimum": 1,
                    "description": "Search depth in plies. Defaults to a fixed depth if omitted."
                },
                "movetime_ms": {
                    "type": "integer", "minimum": 1,
                    "description": "Search time budget in milliseconds (optional)."
                }
            },
            "required": ["fen"]
        }),
        |app, args| async move { engine_analyse(app, args).await },
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

    let fen = match args.get("fen").and_then(Value::as_str) {
        Some(fen) if !fen.trim().is_empty() => fen.to_string(),
        _ => return ToolOutcome::error("Invalid arguments: missing string field `fen`."),
    };

    let limits = Limits {
        depth: args.get("depth").and_then(Value::as_u64).map(|d| d as u32),
        movetime_ms: args.get("movetime_ms").and_then(Value::as_u64),
        nodes: None,
    };

    match service.analyse(&fen, &limits, &BTreeMap::new()).await {
        Ok(analysis) => match serde_json::to_string_pretty(&analysis) {
            Ok(text) => ToolOutcome::ok(text),
            Err(e) => ToolOutcome::error(format!("failed to serialise analysis: {e}")),
        },
        Err(e) => ToolOutcome::error(format!("engine analysis failed: {e}")),
    }
}

// --- JSON-RPC framing ----------------------------------------------------

#[derive(Deserialize)]
struct JsonRpcRequest {
    #[allow(dead_code)]
    jsonrpc: String,
    #[serde(default)]
    id: Option<Value>,
    method: String,
    #[serde(default)]
    params: Option<Value>,
}

#[derive(Serialize)]
struct JsonRpcResponse {
    jsonrpc: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    id: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<JsonRpcError>,
}

#[derive(Serialize)]
struct JsonRpcError {
    code: i32,
    message: String,
}

impl JsonRpcResponse {
    fn success(id: Option<Value>, result: Value) -> Self {
        Self {
            jsonrpc: "2.0",
            id,
            result: Some(result),
            error: None,
        }
    }

    fn error(id: Option<Value>, code: i32, message: impl Into<String>) -> Self {
        Self {
            jsonrpc: "2.0",
            id,
            result: None,
            error: Some(JsonRpcError {
                code,
                message: message.into(),
            }),
        }
    }
}

// --- Dispatch ------------------------------------------------------------

async fn handle(State(state): State<McpState>, Json(req): Json<JsonRpcRequest>) -> Response {
    // `notifications/initialized` is a fire-and-forget notification (no id);
    // acknowledge with 202 and an empty body per the MCP HTTP transport.
    if req.method == "notifications/initialized" {
        return StatusCode::ACCEPTED.into_response();
    }

    let resp = match req.method.as_str() {
        "initialize" => JsonRpcResponse::success(req.id, initialize_result()),
        "tools/list" => JsonRpcResponse::success(req.id, state.registry.list()),
        "tools/call" => tools_call(&state, req.id, req.params).await,
        other => JsonRpcResponse::error(req.id, -32601, format!("Method not found: {other}")),
    };

    (StatusCode::OK, Json(resp)).into_response()
}

fn initialize_result() -> Value {
    json!({
        "protocolVersion": PROTOCOL_VERSION,
        "capabilities": { "tools": {} },
        "serverInfo": {
            "name": SERVER_NAME,
            "version": SERVER_VERSION
        },
        "instructions": INSTRUCTIONS
    })
}

async fn tools_call(state: &McpState, id: Option<Value>, params: Option<Value>) -> JsonRpcResponse {
    let params = match params {
        Some(p) => p,
        None => return JsonRpcResponse::error(id, -32602, "Missing params"),
    };

    let name = params.get("name").and_then(Value::as_str).unwrap_or("");
    let arguments = params
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| json!({}));

    let tool = match state.registry.find(name) {
        Some(t) => t,
        None => return JsonRpcResponse::error(id, -32602, format!("Unknown tool: {name}")),
    };

    let outcome = (tool.handler)(state.app.clone(), arguments).await;
    JsonRpcResponse::success(id, tool_envelope(outcome))
}

/// Wrap a [`ToolOutcome`] into the MCP `tools/call` result envelope.
fn tool_envelope(outcome: ToolOutcome) -> Value {
    let mut result = json!({
        "content": [{ "type": "text", "text": outcome.text }]
    });
    if outcome.is_error {
        result["isError"] = json!(true);
    }
    result
}

// --- Server instructions -------------------------------------------------

/// Returned by `initialize`. Documents the tool surface Epic 9 plugs in and
/// the `<pgn>` / `<fen>` board directives studies render.
const INSTRUCTIONS: &str = "\
# chess-base — MCP Integration

Self-hosted ChessBase replacement. Collect, search and study chess games with \
engine analysis and AI-assisted studies. This endpoint exposes chess tooling \
over JSON-RPC; the available tools depend on what is registered (call \
`tools/list`).

## Tool surface (Epic 9)

- **Engine** — request Stockfish/Lc0 evaluation of a position (best move, score, \
  principal variation) to use as ground truth when annotating.
- **Database** — search the caller's databases and the global ones by game \
  header or by position (64-bit Zobrist hash), and read individual games.
- **Interactive analysis** — walk a study move-tree, play moves, and inspect \
  resulting positions.

Study *mutation* (creating/annotating studies) is a separate programmatic API, \
not an MCP tool.

## Board directives

When writing study text, embed positions and games with these directives:

- `<fen>FEN string</fen>` — render a static board from an inline FEN.
- `<pgn move=\"N\">PGN moves</pgn>` — render a playable game from inline PGN, \
  opened at half-move N.

Always ground evaluations and variations in the engine and database tools \
rather than asserting them unverified.
";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_lists_registered_tools() {
        let registry = default_registry();
        let list = registry.list();
        let tools = list["tools"].as_array().expect("tools array");
        assert!(tools.iter().any(|t| t["name"] == "echo"));
        assert_eq!(list["tools"][0]["inputSchema"]["type"], "object");
    }

    #[test]
    fn engine_tool_is_registered_with_fen_input() {
        let registry = default_registry();
        let tools = registry.list();
        let tools = tools["tools"].as_array().expect("tools array");
        let engine = tools
            .iter()
            .find(|t| t["name"] == "engine_analyse")
            .expect("engine_analyse tool registered");
        assert_eq!(engine["inputSchema"]["required"][0], "fen");
    }

    #[test]
    fn unknown_tool_is_none() {
        assert!(default_registry().find("nope").is_none());
    }

    #[test]
    fn error_outcome_sets_is_error_flag() {
        let env = tool_envelope(ToolOutcome::error("boom"));
        assert_eq!(env["isError"], json!(true));
        assert_eq!(env["content"][0]["text"], "boom");
    }

    #[test]
    fn ok_outcome_omits_is_error_flag() {
        let env = tool_envelope(ToolOutcome::ok("hi"));
        assert!(env.get("isError").is_none());
        assert_eq!(env["content"][0]["type"], "text");
    }
}
