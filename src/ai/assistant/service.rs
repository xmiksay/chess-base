//! The agent loop. Drives a session forward: ask the provider, execute the
//! auto-approved (read-only) tool calls, and **pause** the moment the model wants
//! a mutating tool — the user approves or denies before it runs (issue #20). The
//! loop is bounded by [`MAX_ITERATIONS`] tool rounds per user message.
//!
//! It reuses the in-process [`ToolRegistry`] — the *same* tool surface the `/mcp`
//! transport serves — so there is no second tool implementation.

use std::collections::HashMap;
use std::sync::Arc;

use crate::ai::llm::{CompletionRequest, LlmProvider, Message, ToolCall, ToolResult, ToolSpec};
use crate::db::entities::assistant_sessions;
use crate::server::identity::CurrentUser;
use crate::server::routes::mcp::ToolRegistry;
use crate::server::state::AppState;

use super::{
    build_view, iterations_since_user, last_unanswered_calls, requires_approval, tool_specs,
    AssistantError, AssistantStore, SessionView, CAP_NOTE, DEFAULT_TITLE, MAX_ITERATIONS,
    MAX_TOKENS, SYSTEM_PROMPT,
};

/// Builds and drives assistant sessions over a fixed provider + tool registry.
#[derive(Clone)]
pub struct AssistantService {
    app: AppState,
    provider: Arc<dyn LlmProvider>,
    registry: Arc<ToolRegistry>,
    store: AssistantStore,
    specs: Vec<ToolSpec>,
}

impl AssistantService {
    pub fn new(app: AppState, provider: Arc<dyn LlmProvider>, registry: Arc<ToolRegistry>) -> Self {
        let store = AssistantStore::new(app.db.clone());
        let specs = tool_specs(&registry);
        Self {
            app,
            provider,
            registry,
            store,
            specs,
        }
    }

    /// Start an empty session. `model` defaults to the provider's default.
    pub async fn create_session(
        &self,
        user: &CurrentUser,
        title: Option<String>,
        model: Option<String>,
    ) -> Result<SessionView, AssistantError> {
        let title = title
            .map(|t| t.trim().to_string())
            .filter(|t| !t.is_empty())
            .unwrap_or_else(|| DEFAULT_TITLE.to_string());
        let model = model.unwrap_or_else(|| self.provider.default_model().to_string());
        let session = self.store.create(user, title, model).await?;
        Ok(build_view(&session, &[]))
    }

    /// The current state of a session the caller owns.
    pub async fn view(&self, user: &CurrentUser, id: i32) -> Result<SessionView, AssistantError> {
        let session = self.store.get_owned(user, id).await?;
        let messages = self.store.load_messages(id).await?;
        Ok(build_view(&session, &messages))
    }

    /// Post a user message and run the loop until it answers, pauses for approval,
    /// or hits the iteration cap. Rejected while a previous turn is awaiting an
    /// approval (resolve that first via [`respond`](Self::respond)).
    pub async fn post_message(
        &self,
        user: &CurrentUser,
        id: i32,
        text: &str,
    ) -> Result<SessionView, AssistantError> {
        let session = self.store.get_owned(user, id).await?;
        let messages = self.store.load_messages(id).await?;
        if last_unanswered_calls(&messages).is_some() {
            return Err(AssistantError::Conflict(
                "resolve the pending tool approval before sending a new message".to_string(),
            ));
        }
        self.store.append(id, &Message::user(text)).await?;
        self.drive(user, &session).await?;
        self.view(user, id).await
    }

    /// Resolve a pending approval: run the approved mutating calls (and any
    /// read-only calls in the same batch), record denials, then continue the loop.
    pub async fn respond(
        &self,
        user: &CurrentUser,
        id: i32,
        decisions: HashMap<String, bool>,
    ) -> Result<SessionView, AssistantError> {
        let session = self.store.get_owned(user, id).await?;
        let messages = self.store.load_messages(id).await?;
        let pending = last_unanswered_calls(&messages).ok_or_else(|| {
            AssistantError::Conflict("no pending tool approval for this session".to_string())
        })?;
        let results = self.execute_calls(user, &pending, Some(&decisions)).await;
        self.store
            .append(id, &Message::ToolResults { results })
            .await?;
        self.drive(user, &session).await?;
        self.view(user, id).await
    }

    /// The core loop. Each pass: bail if paused/capped, else ask the provider,
    /// record the turn, and either finish (no tool calls), pause (a mutating call
    /// needs approval) or run the read-only calls and go again.
    async fn drive(
        &self,
        user: &CurrentUser,
        session: &assistant_sessions::Model,
    ) -> Result<(), AssistantError> {
        loop {
            let messages = self.store.load_messages(session.id).await?;
            // A trailing unanswered assistant turn means we are paused for approval.
            if last_unanswered_calls(&messages).is_some() {
                return Ok(());
            }
            if iterations_since_user(&messages) >= MAX_ITERATIONS {
                self.store
                    .append(
                        session.id,
                        &Message::Assistant {
                            text: Some(CAP_NOTE.to_string()),
                            tool_calls: Vec::new(),
                        },
                    )
                    .await?;
                return Ok(());
            }

            let req = CompletionRequest::new(session.model.clone(), messages)
                .with_system(SYSTEM_PROMPT)
                .with_tools(self.specs.clone())
                .with_max_tokens(MAX_TOKENS);
            let resp = self.provider.complete(req).await?;
            let calls = resp.tool_calls.clone();
            self.store
                .append(
                    session.id,
                    &Message::Assistant {
                        text: resp.text,
                        tool_calls: calls.clone(),
                    },
                )
                .await?;

            if calls.is_empty() {
                return Ok(()); // final answer
            }
            if calls.iter().any(|c| requires_approval(&c.name)) {
                return Ok(()); // pause: a mutating call needs approval
            }
            // All read-only: run them and continue.
            let results = self.execute_calls(user, &calls, None).await;
            self.store
                .append(session.id, &Message::ToolResults { results })
                .await?;
        }
    }

    /// Run a batch of tool calls into results. A read-only call always runs; a
    /// mutating call runs only if `decisions` approved it (otherwise a denial
    /// result is recorded). A tool failure becomes an `is_error` result — never a
    /// hard error — so the model can see it and recover.
    async fn execute_calls(
        &self,
        user: &CurrentUser,
        calls: &[ToolCall],
        decisions: Option<&HashMap<String, bool>>,
    ) -> Vec<ToolResult> {
        let mut out = Vec::with_capacity(calls.len());
        for call in calls {
            let approved = if requires_approval(&call.name) {
                decisions
                    .and_then(|d| d.get(&call.id))
                    .copied()
                    .unwrap_or(false)
            } else {
                true
            };
            let result = if !approved {
                ToolResult {
                    tool_call_id: call.id.clone(),
                    content: "The user denied this action.".to_string(),
                    is_error: true,
                }
            } else {
                match self
                    .registry
                    .invoke(
                        &call.name,
                        self.app.clone(),
                        user.clone(),
                        call.input.clone(),
                    )
                    .await
                {
                    Some(outcome) => ToolResult {
                        tool_call_id: call.id.clone(),
                        content: outcome.text,
                        is_error: outcome.is_error,
                    },
                    None => ToolResult {
                        tool_call_id: call.id.clone(),
                        content: format!("unknown tool: {}", call.name),
                        is_error: true,
                    },
                }
            };
            out.push(result);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    use async_trait::async_trait;
    use serde_json::json;

    use crate::ai::llm::{CompletionResponse, ProviderError};
    use crate::db::config::{Backend, DbConfig};
    use crate::server::config::Mode;
    use crate::server::routes::mcp::{Tool, ToolOutcome};

    /// A provider that replays a fixed queue of responses, one per `complete`.
    struct StubProvider {
        responses: Mutex<std::collections::VecDeque<CompletionResponse>>,
    }

    impl StubProvider {
        fn new(responses: Vec<CompletionResponse>) -> Self {
            Self {
                responses: Mutex::new(responses.into()),
            }
        }
    }

    #[async_trait]
    impl LlmProvider for StubProvider {
        async fn complete(
            &self,
            _req: CompletionRequest,
        ) -> Result<CompletionResponse, ProviderError> {
            self.responses
                .lock()
                .expect("stub lock")
                .pop_front()
                .ok_or_else(|| ProviderError::Protocol("stub exhausted".to_string()))
        }
        fn name(&self) -> &'static str {
            "stub"
        }
        fn default_model(&self) -> &str {
            "stub-model"
        }
    }

    fn assistant_calls(calls: Vec<ToolCall>) -> CompletionResponse {
        CompletionResponse {
            text: None,
            tool_calls: calls,
            usage: None,
        }
    }

    fn assistant_text(text: &str) -> CompletionResponse {
        CompletionResponse {
            text: Some(text.to_string()),
            tool_calls: Vec::new(),
            usage: None,
        }
    }

    fn call(id: &str, name: &str) -> ToolCall {
        ToolCall {
            id: id.to_string(),
            name: name.to_string(),
            input: json!({}),
        }
    }

    /// A registry with one read-only and one mutating stub tool, so the loop can
    /// exercise auto-run + approval without a real engine/DB-backed tool.
    fn stub_registry() -> ToolRegistry {
        let mut registry = ToolRegistry::new();
        registry.register(Tool::new(
            "engine_analyse",
            "stub read-only tool",
            json!({"type": "object"}),
            |_app, _user, _args| async move { ToolOutcome::ok("eval: +0.3") },
        ));
        registry.register(Tool::new(
            "study_create",
            "stub mutating tool",
            json!({"type": "object"}),
            |_app, _user, _args| async move { ToolOutcome::ok(json!({"id": 7}).to_string()) },
        ));
        registry
    }

    async fn app_state() -> AppState {
        let db = crate::db::connect(&DbConfig {
            backend: Backend::Sqlite {
                path: ":memory:".to_string(),
            },
        })
        .await
        .expect("connect in-memory db");
        AppState {
            db,
            mode: Mode::Local,
            engine_service: None,
            llm_provider: None,
        }
    }

    #[tokio::test]
    async fn loop_runs_reads_then_pauses_for_approval_then_resumes() {
        let user = CurrentUser::local_admin();
        let app = app_state().await;
        // 1) request a read-only tool, 2) request a mutating tool (pause),
        // 3) after approval, answer.
        let provider = Arc::new(StubProvider::new(vec![
            assistant_calls(vec![call("c1", "engine_analyse")]),
            assistant_calls(vec![call("c2", "study_create")]),
            assistant_text("Created your Sicilian repertoire study."),
        ]));
        let service = AssistantService::new(app, provider, Arc::new(stub_registry()));

        let session = service
            .create_session(&user, Some("Sicilian".to_string()), None)
            .await
            .expect("create");
        assert_eq!(session.model, "stub-model");

        // Posting runs the read tool automatically, then pauses on study_create.
        let view = service
            .post_message(&user, session.id, "build me a repertoire vs the Sicilian")
            .await
            .expect("post");
        assert!(view.awaiting_approval, "loop should pause for approval");
        assert_eq!(view.pending_approvals.len(), 1);
        assert_eq!(view.pending_approvals[0].name, "study_create");
        // The read-only tool already ran (an assistant turn + its results recorded).
        assert!(view
            .messages
            .iter()
            .any(|m| m.tool_results.iter().any(|r| r.content == "eval: +0.3")));

        // Approve the mutating call; the loop resumes and answers.
        let decisions = HashMap::from([("c2".to_string(), true)]);
        let done = service
            .respond(&user, session.id, decisions)
            .await
            .expect("respond");
        assert!(!done.awaiting_approval);
        let last = done.messages.last().expect("a final message");
        assert_eq!(last.role, "assistant");
        assert_eq!(
            last.text.as_deref(),
            Some("Created your Sicilian repertoire study.")
        );
    }

    #[tokio::test]
    async fn denied_mutation_records_an_error_and_continues() {
        let user = CurrentUser::local_admin();
        let app = app_state().await;
        let provider = Arc::new(StubProvider::new(vec![
            assistant_calls(vec![call("c1", "study_create")]),
            assistant_text("Okay, I won't create it."),
        ]));
        let service = AssistantService::new(app, provider, Arc::new(stub_registry()));
        let session = service
            .create_session(&user, None, None)
            .await
            .expect("create");
        let view = service
            .post_message(&user, session.id, "make a study")
            .await
            .expect("post");
        assert!(view.awaiting_approval);

        let decisions = HashMap::from([("c1".to_string(), false)]);
        let done = service
            .respond(&user, session.id, decisions)
            .await
            .expect("respond");
        assert!(!done.awaiting_approval);
        // The denial is recorded as an error tool result.
        assert!(done
            .messages
            .iter()
            .any(|m| m.tool_results.iter().any(|r| r.is_error)));
    }
}
