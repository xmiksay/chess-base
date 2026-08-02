//! Provider-agnostic LLM client abstraction.
//!
//! A small [`LlmProvider`] trait fronts concrete providers. The single entry
//! point is [`LlmProvider::complete`]: a chat completion over a list of
//! [`Message`]s with optional [`ToolSpec`] tool-calling, used by the batch
//! annotation pass (Epic 9) and its test stubs.
//!
//! The old in-crate Anthropic client (and its `Transport` HTTP seam) was
//! removed with the hand-rolled assistant (#198); the concrete client is
//! [`entanglement::StackLlmProvider`], an adapter over the embedded
//! entanglement provider stack resolved per calling user
//! (`AppState::llm_for`).
//!
//! **The API key is server-side only.** Providers are constructed in the backend
//! and never serialized to the SPA.

pub mod entanglement;

use std::time::Duration;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Default per-response output cap. Matches the Messages API guidance for
/// non-streaming requests (keeps responses under the HTTP timeout).
pub const DEFAULT_MAX_TOKENS: u32 = 16_000;

/// A tool the model may call. `input_schema` is a JSON Schema object describing
/// the tool's arguments.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
}

/// One conversational turn, provider-agnostic. Concrete providers translate
/// these into their own wire format.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "role", rename_all = "snake_case")]
pub enum Message {
    User {
        text: String,
    },
    Assistant {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        text: Option<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        tool_calls: Vec<ToolCall>,
    },
    /// Results for the tool calls requested in the preceding assistant turn.
    ToolResults {
        results: Vec<ToolResult>,
    },
}

impl Message {
    pub fn user(text: impl Into<String>) -> Self {
        Message::User { text: text.into() }
    }
}

/// A model's request to call a tool.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub input: Value,
}

/// The caller's result for one [`ToolCall`], fed back on the next turn.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ToolResult {
    pub tool_call_id: String,
    pub content: String,
    #[serde(default)]
    pub is_error: bool,
}

/// One chat-completion request. Build with [`CompletionRequest::new`] and the
/// chainable setters.
#[derive(Clone, Debug)]
pub struct CompletionRequest {
    pub model: String,
    pub system: String,
    pub messages: Vec<Message>,
    pub tools: Vec<ToolSpec>,
    pub max_tokens: u32,
}

impl CompletionRequest {
    pub fn new(model: impl Into<String>, messages: Vec<Message>) -> Self {
        Self {
            model: model.into(),
            system: String::new(),
            messages,
            tools: Vec::new(),
            max_tokens: DEFAULT_MAX_TOKENS,
        }
    }

    pub fn with_system(mut self, system: impl Into<String>) -> Self {
        self.system = system.into();
        self
    }

    pub fn with_tools(mut self, tools: Vec<ToolSpec>) -> Self {
        self.tools = tools;
        self
    }

    pub fn with_max_tokens(mut self, max_tokens: u32) -> Self {
        self.max_tokens = max_tokens;
        self
    }
}

/// Token accounting reported by the provider.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Usage {
    pub input_tokens: i64,
    pub output_tokens: i64,
}

/// A completed response: any free text the model produced, plus any tool calls
/// it requested (both may be present in one turn).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CompletionResponse {
    pub text: Option<String>,
    pub tool_calls: Vec<ToolCall>,
    pub usage: Option<Usage>,
}

#[derive(Debug, thiserror::Error)]
pub enum ProviderError {
    #[error("provider config error: {0}")]
    Config(String),
    #[error("provider transport error: {0}")]
    Transport(String),
    #[error("provider protocol error: {0}")]
    Protocol(String),
    #[error("provider API error (HTTP {status}): {message}")]
    Api { status: u16, message: String },
    /// The provider returned `429`. Callers should back off, honoring
    /// `retry_after` when present.
    #[error("provider rate limited (retry_after={retry_after:?})")]
    RateLimited { retry_after: Option<Duration> },
}

/// Provider-agnostic chat-completion client.
#[async_trait]
pub trait LlmProvider: Send + Sync {
    /// Run one completion. `req.tools` may be empty (no tool-calling).
    async fn complete(&self, req: CompletionRequest) -> Result<CompletionResponse, ProviderError>;

    /// Stable provider identifier (e.g. `"anthropic"`).
    fn name(&self) -> &'static str;

    /// The model used when a request doesn't override it.
    fn default_model(&self) -> &str;
}
