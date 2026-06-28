//! MCP server: a hand-rolled JSON-RPC 2.0 endpoint at `POST /mcp` (ADR-0008).
//!
//! This module is the **transport + dispatch plumbing only**. It owns a
//! [`ToolRegistry`] that the Epic 9 services (engine facade, DB layer,
//! interactive analysis) plug their tools into — each tool is a name +
//! input-schema + async handler. The handler returns a [`ToolOutcome`] which
//! this layer wraps into the MCP content/`isError` envelope. Mirrors the
//! `site` project's proven pattern; no MCP server crate.

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

/// The default registry. A single `echo` stub proves dispatch end-to-end; the
/// Epic 9 services append their real tools here.
fn default_registry() -> ToolRegistry {
    let mut registry = ToolRegistry::new();
    registry.register(echo_tool());
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
