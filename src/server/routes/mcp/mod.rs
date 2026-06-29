//! MCP server: a hand-rolled JSON-RPC 2.0 endpoint at `POST /mcp` (ADR-0008).
//!
//! This module is the **transport + dispatch plumbing**: it authenticates the
//! caller (ADR-0016), owns a [`ToolRegistry`] that the Epic 9 services plug their
//! tools into — each tool is a name + input-schema + async handler — and wraps the
//! handler's [`ToolOutcome`] into the MCP content/`isError` envelope. Every call
//! is authenticated up front; the resolved [`CurrentUser`] is threaded into each
//! handler so a tool scopes its reads/writes to the caller (ADR 0007/0011). The
//! tool builders themselves live in [`tools`].

mod analysis;
mod db_tools;
mod tools;

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use axum::{
    extract::State,
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    routing::post,
    Json, Router,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::server::auth::{authenticate_mcp, BearerChallenge};
use crate::server::identity::CurrentUser;
use crate::server::state::AppState;

const SERVER_NAME: &str = "chess-base";
const SERVER_VERSION: &str = env!("CARGO_PKG_VERSION");
const PROTOCOL_VERSION: &str = "2025-03-26";

/// Mount the `/mcp` endpoint with the default tool registry.
pub fn router(app: AppState) -> Router {
    let state = McpState {
        app,
        registry: Arc::new(tools::default_registry()),
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

/// Boxed async tool handler: `(app state, caller, arguments) -> outcome`.
type ToolFuture = Pin<Box<dyn Future<Output = ToolOutcome> + Send>>;
type ToolFn = Arc<dyn Fn(AppState, CurrentUser, Value) -> ToolFuture + Send + Sync>;

/// A registered tool: its `tools/list` metadata plus the dispatch handler.
pub struct Tool {
    pub name: &'static str,
    pub description: &'static str,
    pub input_schema: Value,
    handler: ToolFn,
}

impl Tool {
    /// Build a tool from metadata and an async handler closure. The handler
    /// receives the cloned [`AppState`], the resolved [`CurrentUser`], and the raw
    /// `arguments` object.
    pub fn new<F, Fut>(
        name: &'static str,
        description: &'static str,
        input_schema: Value,
        handler: F,
    ) -> Self
    where
        F: Fn(AppState, CurrentUser, Value) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = ToolOutcome> + Send + 'static,
    {
        Self {
            name,
            description,
            input_schema,
            handler: Arc::new(move |state, user, args| Box::pin(handler(state, user, args))),
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
    pub fn list(&self) -> Value {
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

async fn handle(
    State(state): State<McpState>,
    headers: HeaderMap,
    Json(req): Json<JsonRpcRequest>,
) -> Response {
    // Every `/mcp` call is authenticated; an OAuth access token or a service
    // token resolves the caller, otherwise a 401 + bearer challenge points the
    // client at OAuth discovery.
    let user = match authenticate_mcp(&state.app, &headers).await {
        Ok(user) => user,
        Err(challenge) => return unauthorized(challenge),
    };

    // `notifications/initialized` is a fire-and-forget notification (no id);
    // acknowledge with 202 and an empty body per the MCP HTTP transport.
    if req.method == "notifications/initialized" {
        return StatusCode::ACCEPTED.into_response();
    }

    let resp = match req.method.as_str() {
        "initialize" => JsonRpcResponse::success(req.id, initialize_result()),
        "tools/list" => JsonRpcResponse::success(req.id, state.registry.list()),
        "tools/call" => tools_call(&state, &user, req.id, req.params).await,
        other => JsonRpcResponse::error(req.id, -32601, format!("Method not found: {other}")),
    };

    (StatusCode::OK, Json(resp)).into_response()
}

/// Build the `401` response carrying the `WWW-Authenticate` bearer challenge.
fn unauthorized(challenge: BearerChallenge) -> Response {
    let body = Json(JsonRpcResponse::error(None, -32000, "Unauthorized"));
    let mut response: Response = (StatusCode::UNAUTHORIZED, body).into_response();
    if let Ok(value) = HeaderValue::from_str(&challenge.www_authenticate) {
        response
            .headers_mut()
            .insert(header::WWW_AUTHENTICATE, value);
    }
    response
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

async fn tools_call(
    state: &McpState,
    user: &CurrentUser,
    id: Option<Value>,
    params: Option<Value>,
) -> JsonRpcResponse {
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

    let outcome = (tool.handler)(state.app.clone(), user.clone(), arguments).await;
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

- **Interactive analysis** — `analyse_position` is the one-shot \"explain this \
  position\" entry point: it bundles engine eval, the database report and factual \
  feature tags for a single FEN so an explanation cites tool output, not guesses. \
  The tools below are the same sources unbundled, for drilling in further.
- **Engine** — request Stockfish/Lc0 evaluation of a position (best move, score, \
  principal variation) to use as ground truth when annotating.
- **Database** — search the caller's databases and the global ones by game \
  header or by position (64-bit Zobrist hash), and read individual games.
- **Studies** — create studies and edit their move-trees (add moves, annotate). \
  Every edit is scoped to the authenticated caller: you may only mutate your own \
  studies (global studies require admin).

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
