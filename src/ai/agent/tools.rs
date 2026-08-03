//! Bridge from the MCP [`ToolRegistry`] to entanglement's tool vocabulary
//! (#198, step 3): every MCP tool is re-exposed 1:1 (same name, description,
//! input schema) as a [`BridgedTool`] the engine's executor can dispatch. The
//! bridge resolves the caller from the session's registered user (the engine
//! runs many users' sessions over one process), invokes the shared MCP handler
//! in-process, and maps [`ToolOutcome`] onto the entanglement result shape —
//! errors become `Err` (fed back to the model), success text is capped at
//! [`MAX_OUTPUT_BYTES`].
//!
//! [`ToolRegistry`]: McpRegistry
//! [`MAX_OUTPUT_BYTES`]: entanglement_runtime::host::MAX_OUTPUT_BYTES

use std::borrow::Cow;
use std::sync::Arc;

use anyhow::{anyhow, bail, Context as _};
use async_trait::async_trait;
use entanglement_core::{ContentPart, SessionId};
use entanglement_runtime::host::truncate_output;
use entanglement_runtime::multi_user::SessionUserRegistry;
use entanglement_runtime::tools::{Tool, ToolRegistry};
use sea_orm::EntityTrait;
use serde_json::{json, Value};

use crate::db::entities::users;
use crate::server::config::Mode;
use crate::server::identity::CurrentUser;
use crate::server::routes::mcp::{default_registry, ToolRegistry as McpRegistry};
use crate::server::state::AppState;

/// One MCP tool re-exposed to the entanglement engine. Sessionful only: the
/// caller's identity comes from the session's registered user, so the plain
/// [`Tool::run`] path (no session) is refused.
pub struct BridgedTool {
    name: &'static str,
    description: &'static str,
    schema: Value,
    app: AppState,
    users: SessionUserRegistry,
    registry: Arc<McpRegistry>,
}

impl BridgedTool {
    /// Resolve the session's [`CurrentUser`]. Local mode has exactly one
    /// implicit admin; server mode loads the registered user's row (a raw
    /// `DbErr` is logged, never surfaced to the model).
    async fn resolve_caller(&self, session: &SessionId) -> anyhow::Result<CurrentUser> {
        let user = self
            .users
            .user_for(session)
            .ok_or_else(|| anyhow!("no user registered for this session"))?;
        match self.app.mode {
            Mode::Local => Ok(CurrentUser::local_admin()),
            Mode::Server => {
                let row = users::Entity::find_by_id(user.0.clone())
                    .one(&self.app.db)
                    .await
                    .map_err(|err| {
                        tracing::warn!(%session, error = %err, "agent caller lookup failed");
                        anyhow!("could not resolve caller")
                    })?
                    .ok_or_else(|| anyhow!("unknown user: {}", user.0))?;
                Ok(CurrentUser {
                    id: row.id,
                    is_admin: row.is_admin,
                    public: false,
                })
            }
        }
    }
}

/// Parse the model's raw input into the arguments object the MCP handlers
/// take. Only a fully-empty input defaults to `{}` (a schema-less call);
/// malformed non-empty JSON errs with the parse message, which is model-facing
/// and lets it self-correct.
fn parse_args(input: &str) -> anyhow::Result<Value> {
    if input.trim().is_empty() {
        return Ok(json!({}));
    }
    serde_json::from_str(input).context("parsing tool input as JSON")
}

#[async_trait]
impl Tool for BridgedTool {
    fn name(&self) -> Cow<'static, str> {
        Cow::Borrowed(self.name)
    }

    fn description(&self) -> &str {
        self.description
    }

    fn schema(&self) -> Value {
        self.schema.clone()
    }

    async fn run(&self, _input: &str) -> anyhow::Result<String> {
        bail!("chess-base tools require a session context")
    }

    async fn run_for_session(
        &self,
        session: &SessionId,
        _request_id: &str,
        input: &str,
    ) -> anyhow::Result<Vec<ContentPart>> {
        let caller = self.resolve_caller(session).await?;
        let args = parse_args(input)?;
        let outcome = self
            .registry
            .invoke(self.name, self.app.clone(), caller, args)
            .await
            .ok_or_else(|| anyhow!("unknown tool: {}", self.name))?;
        if outcome.is_error {
            bail!(outcome.text);
        }
        let text = truncate_output(outcome.text);
        Ok(if text.is_empty() {
            Vec::new()
        } else {
            vec![ContentPart::text(text)]
        })
    }
}

/// The entanglement registry mirroring the full MCP surface 1:1.
pub fn bridge_registry(app: &AppState, users: &SessionUserRegistry) -> ToolRegistry {
    bridge_from(Arc::new(default_registry()), app, users)
}

fn bridge_from(mcp: Arc<McpRegistry>, app: &AppState, users: &SessionUserRegistry) -> ToolRegistry {
    let mut registry = ToolRegistry::new();
    for tool in mcp.tools() {
        registry.register(BridgedTool {
            name: tool.name,
            description: tool.description,
            schema: tool.input_schema.clone(),
            app: app.clone(),
            users: users.clone(),
            registry: Arc::clone(&mcp),
        });
    }
    registry
}

#[cfg(test)]
mod tests;
