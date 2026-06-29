//! Anthropic Messages API provider.
//!
//! Implements [`LlmProvider`] over the `POST /v1/messages` endpoint. The model
//! id is configurable; the default is a Sonnet-class model (cost-effective for
//! the high-volume batch annotation pass), with Opus available by overriding it.
//!
//! The API key is held server-side and sent only as the `x-api-key` request
//! header — it never appears in [`CompletionResponse`] or any client-facing type.

use std::time::{Duration, Instant};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::{
    CompletionRequest, CompletionResponse, HttpRequest, HttpResponse, LlmProvider, Message,
    ProviderError, ToolCall, ToolSpec, Transport, Usage,
};

const ANTHROPIC_API_URL: &str = "https://api.anthropic.com/v1/messages";
const ANTHROPIC_VERSION: &str = "2023-06-01";

/// Default model: a current Sonnet-class id. Override per-request (or via
/// [`AnthropicProvider::with_model`]) to use Opus for higher-stakes work.
pub const DEFAULT_MODEL: &str = "claude-sonnet-4-6";

/// Anthropic Messages API client, generic over the HTTP [`Transport`] so tests
/// can inject a stub. The default transport is [`ReqwestTransport`].
pub struct AnthropicProvider<T: Transport = ReqwestTransport> {
    api_key: String,
    default_model: String,
    transport: T,
}

impl AnthropicProvider<ReqwestTransport> {
    /// Build a provider using the [`DEFAULT_MODEL`] and a real HTTP transport.
    pub fn new(api_key: String) -> Result<Self, ProviderError> {
        Self::with_model(api_key, DEFAULT_MODEL.to_string())
    }

    /// Build a provider with an explicit default model and a real HTTP transport.
    pub fn with_model(api_key: String, default_model: String) -> Result<Self, ProviderError> {
        Ok(Self {
            api_key,
            default_model,
            transport: ReqwestTransport::new()?,
        })
    }
}

impl<T: Transport> AnthropicProvider<T> {
    /// Build a provider over an arbitrary transport (used by tests).
    pub fn with_transport(api_key: String, default_model: String, transport: T) -> Self {
        Self {
            api_key,
            default_model,
            transport,
        }
    }

    fn build_http_request(&self, req: &CompletionRequest) -> Result<HttpRequest, ProviderError> {
        let wire = AnthropicRequest {
            model: req.model.clone(),
            max_tokens: req.max_tokens,
            system: if req.system.is_empty() {
                None
            } else {
                Some(req.system.clone())
            },
            messages: convert_messages(&req.messages),
            tools: convert_tools(&req.tools),
        };
        let body = serde_json::to_vec(&wire)
            .map_err(|e| ProviderError::Protocol(format!("serialize request: {e}")))?;
        Ok(HttpRequest {
            url: ANTHROPIC_API_URL.to_string(),
            headers: vec![
                ("x-api-key".to_string(), self.api_key.clone()),
                (
                    "anthropic-version".to_string(),
                    ANTHROPIC_VERSION.to_string(),
                ),
                ("content-type".to_string(), "application/json".to_string()),
            ],
            body,
        })
    }
}

#[async_trait]
impl<T: Transport> LlmProvider for AnthropicProvider<T> {
    async fn complete(&self, req: CompletionRequest) -> Result<CompletionResponse, ProviderError> {
        let http_req = self.build_http_request(&req)?;

        let start = Instant::now();
        let resp = self.transport.execute(http_req).await?;

        if resp.status == 429 {
            return Err(ProviderError::RateLimited {
                retry_after: resp.retry_after,
            });
        }
        if !resp.is_success() {
            return Err(ProviderError::Api {
                status: resp.status,
                message: String::from_utf8_lossy(&resp.body).into_owned(),
            });
        }

        let parsed: AnthropicResponse = serde_json::from_slice(&resp.body)
            .map_err(|e| ProviderError::Protocol(format!("parse response: {e}")))?;

        let mut text_parts: Vec<String> = Vec::new();
        let mut tool_calls: Vec<ToolCall> = Vec::new();
        for block in parsed.content {
            match block {
                AnthropicContentBlock::Text { text } => text_parts.push(text),
                AnthropicContentBlock::ToolUse { id, name, input } => {
                    tool_calls.push(ToolCall { id, name, input })
                }
                AnthropicContentBlock::Other => {}
            }
        }
        let text = (!text_parts.is_empty()).then(|| text_parts.join("\n"));
        let usage = parsed.usage.map(|u| Usage {
            input_tokens: u.input_tokens,
            output_tokens: u.output_tokens,
        });

        tracing::debug!(
            model = %req.model,
            elapsed_ms = start.elapsed().as_millis() as u64,
            tool_calls = tool_calls.len(),
            "anthropic completion"
        );

        Ok(CompletionResponse {
            text,
            tool_calls,
            usage,
        })
    }

    fn name(&self) -> &'static str {
        "anthropic"
    }

    fn default_model(&self) -> &str {
        &self.default_model
    }
}

// ---------------------------------------------------------------------------
// Real transport
// ---------------------------------------------------------------------------

/// Production [`Transport`] backed by `reqwest`.
pub struct ReqwestTransport {
    http: reqwest::Client,
}

impl ReqwestTransport {
    pub fn new() -> Result<Self, ProviderError> {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(300))
            .build()
            .map_err(|e| ProviderError::Config(format!("build HTTP client: {e}")))?;
        Ok(Self { http })
    }
}

#[async_trait]
impl Transport for ReqwestTransport {
    async fn execute(&self, req: HttpRequest) -> Result<HttpResponse, ProviderError> {
        let mut builder = self.http.post(&req.url);
        for (name, value) in &req.headers {
            builder = builder.header(name, value);
        }
        let resp = builder
            .body(req.body)
            .send()
            .await
            .map_err(|e| ProviderError::Transport(format!("request failed: {e}")))?;

        let status = resp.status().as_u16();
        let retry_after = resp
            .headers()
            .get(reqwest::header::RETRY_AFTER)
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.trim().parse::<u64>().ok())
            .map(Duration::from_secs);
        let body = resp
            .bytes()
            .await
            .map_err(|e| ProviderError::Transport(format!("read body: {e}")))?
            .to_vec();

        Ok(HttpResponse {
            status,
            retry_after,
            body,
        })
    }
}

// ---------------------------------------------------------------------------
// Wire types
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct AnthropicRequest {
    model: String,
    max_tokens: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    system: Option<String>,
    messages: Vec<AnthropicMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<Value>>,
}

#[derive(Serialize)]
struct AnthropicMessage {
    role: &'static str,
    content: Value,
}

#[derive(Deserialize)]
struct AnthropicResponse {
    content: Vec<AnthropicContentBlock>,
    usage: Option<AnthropicUsage>,
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum AnthropicContentBlock {
    Text {
        text: String,
    },
    ToolUse {
        id: String,
        name: String,
        input: Value,
    },
    /// Any other block type (e.g. thinking) we don't surface — ignored.
    #[serde(other)]
    Other,
}

#[derive(Deserialize)]
struct AnthropicUsage {
    input_tokens: i64,
    output_tokens: i64,
}

// ---------------------------------------------------------------------------
// Conversion
// ---------------------------------------------------------------------------

fn convert_messages(messages: &[Message]) -> Vec<AnthropicMessage> {
    let mut out = Vec::with_capacity(messages.len());
    for m in messages {
        match m {
            Message::User { text } => out.push(AnthropicMessage {
                role: "user",
                content: Value::String(text.clone()),
            }),
            Message::Assistant { text, tool_calls } => {
                if tool_calls.is_empty() {
                    if let Some(t) = text {
                        out.push(AnthropicMessage {
                            role: "assistant",
                            content: Value::String(t.clone()),
                        });
                    }
                } else {
                    let mut blocks: Vec<Value> = Vec::new();
                    if let Some(t) = text {
                        if !t.is_empty() {
                            blocks.push(json!({ "type": "text", "text": t }));
                        }
                    }
                    for tc in tool_calls {
                        blocks.push(json!({
                            "type": "tool_use",
                            "id": tc.id,
                            "name": tc.name,
                            "input": tc.input,
                        }));
                    }
                    out.push(AnthropicMessage {
                        role: "assistant",
                        content: Value::Array(blocks),
                    });
                }
            }
            Message::ToolResults { results } => {
                let blocks: Vec<Value> = results
                    .iter()
                    .map(|r| {
                        json!({
                            "type": "tool_result",
                            "tool_use_id": r.tool_call_id,
                            "content": r.content,
                            "is_error": r.is_error,
                        })
                    })
                    .collect();
                out.push(AnthropicMessage {
                    role: "user",
                    content: Value::Array(blocks),
                });
            }
        }
    }
    out
}

fn convert_tools(tools: &[ToolSpec]) -> Option<Vec<Value>> {
    if tools.is_empty() {
        return None;
    }
    Some(
        tools
            .iter()
            .map(|t| {
                json!({
                    "name": t.name,
                    "description": t.description,
                    "input_schema": t.input_schema,
                })
            })
            .collect(),
    )
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[path = "anthropic_tests.rs"]
mod tests;
