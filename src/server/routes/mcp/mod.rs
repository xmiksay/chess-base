//! MCP server: a hand-rolled JSON-RPC 2.0 endpoint at `POST /mcp` (ADR-0008).
//!
//! This module is the **transport + dispatch plumbing**: it authenticates the
//! caller (ADR-0016), owns a [`ToolRegistry`] that the Epic 9 services plug their
//! tools into — each tool is a name + input-schema + async handler — and wraps the
//! handler's [`ToolOutcome`] into the MCP content/`isError` envelope. Every call
//! is authenticated up front; the resolved [`CurrentUser`] is threaded into each
//! handler so a tool scopes its reads/writes to the caller (ADR 0007/0011). A
//! server-mode request with no credential resolves to the anonymous public
//! identity instead of `401` (ADR-0043, issue #192): [`ANONYMOUS_ALLOWLIST`]
//! gates both `tools/list` and `tools/call` down to plain data reads — no
//! engine, no studies, no writes — everything else is scoped to global
//! (`owner_id IS NULL`) rows via `CurrentUser`'s `public` flag. A `read_only`
//! caller (ADR-0044, e.g. a `read_only`/`global_read`-scoped service token)
//! gets the same `tools/call` gate on any tool `ai::agent::requires_approval`
//! flags as mutating, and `tools/list` filters those out too. The tool
//! builders themselves live in [`tools`]; the registry shape lives in
//! [`registry`], JSON-RPC framing in [`rpc`], the `initialize` instructions
//! text in [`instructions`] — split out to keep this file under the
//! file-size cap.

mod analysis;
mod db_export_tools;
mod db_tools;
mod folder_tools;
mod game_tools;
mod import_tools;
mod instructions;
mod preprocess;
mod registry;
mod rpc;
mod search_tools;
mod study_node_tools;
mod study_repertoire_tools;
mod study_tools;
#[cfg(test)]
mod symmetry;
mod tools;

use std::sync::Arc;

use axum::{
    body::Bytes,
    extract::State,
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    routing::post,
    Json, Router,
};
use serde_json::{json, Value};

use crate::server::auth::{authenticate_mcp, BearerChallenge};
use crate::server::identity::CurrentUser;
use crate::server::state::AppState;
use instructions::INSTRUCTIONS;
use rpc::{parse_request, JsonRpcResponse};

const SERVER_NAME: &str = "chess-base";
const SERVER_VERSION: &str = env!("CARGO_PKG_VERSION");
const PROTOCOL_VERSION: &str = "2025-03-26";

/// Tools the anonymous public caller (ADR-0043, issue #192) may invoke: data
/// reads on global databases only, no engine (Stockfish CPU is a DoS surface
/// for an unauthenticated caller), no studies/folders, no writes. `tools/list`
/// filters to this set too, so an anonymous client never sees a tool it can't
/// call.
const ANONYMOUS_ALLOWLIST: &[&str] = &[
    "echo",
    "list_databases",
    "db_list_games",
    "db_read_game",
    "db_position_report",
    "db_reference_games",
    "db_export_games",
    "search_headers",
];

pub use registry::{Tool, ToolOutcome, ToolRegistry};
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
        "tools/list" => {
            let mut list = state.registry.list();
            if user.public {
                restrict_to_allowlist(&mut list);
            }
            if user.read_only {
                restrict_to_non_gated(&mut list);
            }
            JsonRpcResponse::success(req.id, list)
        }
        "tools/call" => tools_call(&state, &user, req.id, req.params).await,
        other => JsonRpcResponse::error(req.id, -32601, format!("Method not found: {other}")),
    };

    (StatusCode::OK, Json(resp)).into_response()
}

/// Filter a `tools/list` result down to [`ANONYMOUS_ALLOWLIST`] in place.
fn restrict_to_allowlist(list: &mut Value) {
    if let Some(tools) = list.get_mut("tools").and_then(Value::as_array_mut) {
        tools.retain(|t| {
            t["name"]
                .as_str()
                .is_some_and(|name| ANONYMOUS_ALLOWLIST.contains(&name))
        });
    }
}

/// Filter a `tools/list` result down to the tools a `read_only` caller
/// (ADR-0044) can actually invoke: everything `ai::agent::requires_approval`
/// doesn't flag as mutating.
fn restrict_to_non_gated(list: &mut Value) {
    if let Some(tools) = list.get_mut("tools").and_then(Value::as_array_mut) {
        tools.retain(|t| {
            t["name"]
                .as_str()
                .is_some_and(|name| !crate::ai::agent::requires_approval(name))
        });
    }
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

    if user.public && !ANONYMOUS_ALLOWLIST.contains(&name) {
        return JsonRpcResponse::error(
            id,
            -32001,
            format!("Authentication required: `{name}` is not available to anonymous callers."),
        );
    }
    if user.read_only && crate::ai::agent::requires_approval(name) {
        return JsonRpcResponse::error(
            id,
            -32001,
            format!("This token is read-only and cannot call `{name}`."),
        );
    }

    let tool = match state.registry.find(name) {
        Some(t) => t,
        None => return JsonRpcResponse::error(id, -32602, format!("Unknown tool: {name}")),
    };

    let outcome = tool
        .invoke(state.app.clone(), user.clone(), arguments)
        .await;
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

    #[test]
    fn restrict_to_allowlist_keeps_only_allowlisted_tools() {
        let mut list = json!({
            "tools": [
                { "name": "echo" },
                { "name": "study_create" },
                { "name": "search_headers" },
                { "name": "engine_analyse" },
            ]
        });
        restrict_to_allowlist(&mut list);
        let names: Vec<&str> = list["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["name"].as_str().unwrap())
            .collect();
        assert_eq!(names, vec!["echo", "search_headers"]);
    }

    #[test]
    fn restrict_to_non_gated_keeps_only_non_mutating_tools() {
        let mut list = json!({
            "tools": [
                { "name": "echo" },
                { "name": "study_create" },
                { "name": "study_get" },
                { "name": "folder_create" },
            ]
        });
        restrict_to_non_gated(&mut list);
        let names: Vec<&str> = list["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["name"].as_str().unwrap())
            .collect();
        assert_eq!(names, vec!["echo", "study_get"]);
    }

    #[test]
    fn anonymous_allowlist_matches_the_issue_192_read_only_data_tools() {
        // Guards the allowlist against silent drift — every entry here is a data
        // read scoped to global databases; nothing engine/study/write-shaped.
        for tool in [
            "echo",
            "list_databases",
            "db_list_games",
            "db_read_game",
            "db_position_report",
            "db_reference_games",
            "db_export_games",
            "search_headers",
        ] {
            assert!(ANONYMOUS_ALLOWLIST.contains(&tool), "missing {tool}");
        }
        assert_eq!(ANONYMOUS_ALLOWLIST.len(), 8);
    }
}
