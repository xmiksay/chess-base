//! The MCP tool registry: a name + input-schema + async handler triple every
//! Epic 9 service registers into, plus the [`ToolOutcome`] envelope handlers
//! return. Split out of [`super`] to keep that file under the file-size cap —
//! dispatch/JSON-RPC framing stays there, this is just the registry shape.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use serde_json::{json, Value};

use crate::server::identity::CurrentUser;
use crate::server::state::AppState;

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

    pub(super) fn find(&self, name: &str) -> Option<&Tool> {
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
