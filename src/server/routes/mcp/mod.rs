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
mod preprocess;
mod study_tools;
mod tools;

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use axum::{
    body::Bytes,
    extract::State,
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    routing::post,
    Json, Router,
};
use serde::Serialize;
use serde_json::{json, Value};

use crate::server::auth::{authenticate_mcp, BearerChallenge};
use crate::server::identity::CurrentUser;
use crate::server::state::AppState;

const SERVER_NAME: &str = "chess-base";
const SERVER_VERSION: &str = env!("CARGO_PKG_VERSION");
const PROTOCOL_VERSION: &str = "2025-03-26";

pub use tools::default_registry;

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

    /// Run this tool's handler. The embedded assistant (issue #20) invokes the
    /// same handlers in-process as the `/mcp` transport does, so one tool surface
    /// backs both — no second implementation.
    pub async fn invoke(&self, app: AppState, user: CurrentUser, args: Value) -> ToolOutcome {
        (self.handler)(app, user, args).await
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

    /// The registered tools, for callers that drive the surface in-process (the
    /// embedded assistant builds its tool specs from these — issue #20).
    pub fn tools(&self) -> &[Tool] {
        &self.tools
    }

    /// Run the named tool, or `None` if no tool by that name is registered.
    pub async fn invoke(
        &self,
        name: &str,
        app: AppState,
        user: CurrentUser,
        args: Value,
    ) -> Option<ToolOutcome> {
        let tool = self.find(name)?;
        Some(tool.invoke(app, user, args).await)
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

/// A validated JSON-RPC request. Built by [`parse_request`] from the raw body so
/// a malformed body yields a framed `-32700`/`-32600` error instead of axum's
/// bare-text `400` (issue #97).
struct JsonRpcRequest {
    id: Option<Value>,
    method: String,
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

/// Parse the raw request body into a validated [`JsonRpcRequest`], or a framed
/// JSON-RPC error response when it is malformed. JSON-RPC clients expect a `200`
/// carrying `{"error":{"code":-32700/-32600,…}}` — not the bare-text `400` axum's
/// `Json` extractor would emit — so invalid JSON maps to `-32700` (parse error)
/// and a structurally-invalid request to `-32600` (invalid request), echoing the
/// caller's `id` when one can be recovered (issue #97).
fn parse_request(body: &[u8]) -> Result<JsonRpcRequest, JsonRpcResponse> {
    let value: Value = serde_json::from_slice(body)
        .map_err(|_| JsonRpcResponse::error(None, -32700, "Parse error"))?;

    // Recover the id even from an otherwise-invalid request so the client can
    // correlate the error; a non-scalar id is not a valid id, so drop it.
    let id = match value.get("id") {
        Some(v @ (Value::String(_) | Value::Number(_) | Value::Null)) => Some(v.clone()),
        _ => None,
    };

    if value.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
        return Err(JsonRpcResponse::error(id, -32600, "Invalid Request"));
    }
    let method = match value.get("method").and_then(Value::as_str) {
        Some(m) => m.to_string(),
        None => return Err(JsonRpcResponse::error(id, -32600, "Invalid Request")),
    };

    // Treat an explicit `null` params the same as an absent one.
    let params = value.get("params").filter(|p| !p.is_null()).cloned();

    Ok(JsonRpcRequest { id, method, params })
}

async fn handle(State(state): State<McpState>, headers: HeaderMap, body: Bytes) -> Response {
    // Every `/mcp` call is authenticated; an OAuth access token or a service
    // token resolves the caller, otherwise a 401 + bearer challenge points the
    // client at OAuth discovery.
    let user = match authenticate_mcp(&state.app, &headers).await {
        Ok(user) => user,
        Err(challenge) => return unauthorized(challenge),
    };

    let req = match parse_request(&body) {
        Ok(req) => req,
        Err(resp) => return (StatusCode::OK, Json(resp)).into_response(),
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
  `analyse_game` is its whole-game counterpart: walk the engine over a PGN for a \
  per-ply eval + best-move + classification review. The tools below are the same \
  sources unbundled, for drilling in further.
- **Engine** — request Stockfish/Lc0 evaluation of a position (best move, score, \
  principal variation) to use as ground truth when annotating.
- **Database** — `list_databases` discovers the collections you can see (with \
  game counts) and the `database_id`s the study tools need; `db_list_games` / \
  `db_read_game` page through and read individual games; `db_position_report` / \
  `db_reference_games` search by position (64-bit Zobrist hash).
- **Study preprocessing** — engine + DB grounded *data* for study building, \
  with no language model inside the tool (you are the model — annotate the \
  output yourself, then persist with the study tools): `opening_tree` builds a \
  pruned, eval- and stats-tagged variation tree (the opening skeleton); \
  `danger_map` walks a repertoire spine PGN into an engine-adjudicated danger \
  tree (Weapon / Caution / Off-book roles); `position_concepts` classifies a \
  position's pawn structure and key squares.
- **Studies** — create studies and edit their move-trees: `study_import_pgn` \
  builds a whole study from PGN in one call, or `study_create` + `study_add_move` \
  (SAN or UCI, with optional inline comment/NAG) build one move at a time; \
  `study_get` reads an existing study's tree (with node ids) so you can \
  `study_annotate` it; `study_export` emits re-importable PGN. Every edit is \
  scoped to the authenticated caller: you may only mutate your own studies \
  (global studies require admin).

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

    fn parse_err(body: &[u8]) -> JsonRpcError {
        parse_request(body)
            .err()
            .expect("expected a framed error")
            .error
            .expect("error envelope")
    }

    #[test]
    fn parse_request_accepts_a_well_formed_request() {
        let Ok(req) = parse_request(br#"{"jsonrpc":"2.0","id":7,"method":"tools/list"}"#) else {
            panic!("expected a valid request");
        };
        assert_eq!(req.method, "tools/list");
        assert_eq!(req.id, Some(json!(7)));
        assert!(req.params.is_none());
    }

    #[test]
    fn parse_request_maps_invalid_json_to_parse_error() {
        let err = parse_err(b"{not json");
        assert_eq!(err.code, -32700);
    }

    #[test]
    fn parse_request_rejects_a_missing_method_with_invalid_request() {
        // The id is still echoed so the client can correlate the error.
        let err = parse_request(br#"{"jsonrpc":"2.0","id":3}"#)
            .err()
            .expect("framed error");
        assert_eq!(err.id, Some(json!(3)));
        assert_eq!(err.error.expect("envelope").code, -32600);
    }

    #[test]
    fn parse_request_rejects_a_wrong_jsonrpc_version() {
        assert_eq!(
            parse_err(br#"{"jsonrpc":"1.0","id":1,"method":"x"}"#).code,
            -32600
        );
    }

    #[test]
    fn parse_request_rejects_a_non_string_method() {
        assert_eq!(
            parse_err(br#"{"jsonrpc":"2.0","id":1,"method":42}"#).code,
            -32600
        );
    }

    #[test]
    fn parse_request_drops_a_non_scalar_id() {
        let resp = parse_request(br#"{"jsonrpc":"2.0","id":{"a":1},"method":42}"#)
            .err()
            .expect("framed error");
        assert!(resp.id.is_none());
    }
}
