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
mod tests {
    use super::*;
    use crate::ai::llm::{ToolResult, DEFAULT_MAX_TOKENS};
    use std::sync::Mutex;

    /// Stub transport: records the request it received and returns a canned
    /// response. No network.
    struct StubTransport {
        response: HttpResponse,
        last_request: Mutex<Option<HttpRequest>>,
    }

    impl StubTransport {
        fn ok(body: Value) -> Self {
            Self {
                response: HttpResponse {
                    status: 200,
                    retry_after: None,
                    body: serde_json::to_vec(&body).unwrap(),
                },
                last_request: Mutex::new(None),
            }
        }

        fn with_response(response: HttpResponse) -> Self {
            Self {
                response,
                last_request: Mutex::new(None),
            }
        }

        fn taken_request(&self) -> HttpRequest {
            self.last_request
                .lock()
                .unwrap()
                .clone()
                .expect("no request recorded")
        }

        fn sent_body(&self) -> Value {
            serde_json::from_slice(&self.taken_request().body).unwrap()
        }
    }

    #[async_trait]
    impl Transport for StubTransport {
        async fn execute(&self, req: HttpRequest) -> Result<HttpResponse, ProviderError> {
            *self.last_request.lock().unwrap() = Some(req);
            Ok(self.response.clone())
        }
    }

    fn provider(transport: StubTransport) -> AnthropicProvider<StubTransport> {
        AnthropicProvider::with_transport(
            "secret-key".to_string(),
            DEFAULT_MODEL.to_string(),
            transport,
        )
    }

    #[tokio::test]
    async fn parses_text_and_usage() {
        let body = json!({
            "content": [{ "type": "text", "text": "Hello there" }],
            "usage": { "input_tokens": 12, "output_tokens": 5 }
        });
        let p = provider(StubTransport::ok(body));
        let resp = p
            .complete(CompletionRequest::new(
                DEFAULT_MODEL,
                vec![Message::user("hi")],
            ))
            .await
            .unwrap();

        assert_eq!(resp.text.as_deref(), Some("Hello there"));
        assert!(resp.tool_calls.is_empty());
        let usage = resp.usage.unwrap();
        assert_eq!(usage.input_tokens, 12);
        assert_eq!(usage.output_tokens, 5);
    }

    #[tokio::test]
    async fn parses_tool_calls_and_ignores_unknown_blocks() {
        let body = json!({
            "content": [
                { "type": "thinking", "thinking": "" },
                { "type": "text", "text": "let me check" },
                { "type": "tool_use", "id": "toolu_1", "name": "lookup", "input": { "fen": "x" } }
            ]
        });
        let p = provider(StubTransport::ok(body));
        let resp = p
            .complete(CompletionRequest::new(
                DEFAULT_MODEL,
                vec![Message::user("hi")],
            ))
            .await
            .unwrap();

        assert_eq!(resp.text.as_deref(), Some("let me check"));
        assert_eq!(resp.tool_calls.len(), 1);
        assert_eq!(resp.tool_calls[0].id, "toolu_1");
        assert_eq!(resp.tool_calls[0].name, "lookup");
        assert_eq!(resp.tool_calls[0].input, json!({ "fen": "x" }));
    }

    #[tokio::test]
    async fn request_carries_api_key_header_and_no_body_leak() {
        let p = provider(StubTransport::ok(json!({ "content": [] })));
        p.complete(
            CompletionRequest::new("claude-opus-4-8", vec![Message::user("hi")])
                .with_system("be terse")
                .with_max_tokens(2048),
        )
        .await
        .unwrap();

        let req = p.transport.taken_request();
        assert_eq!(req.url, ANTHROPIC_API_URL);
        // API key travels only in the header, never in the JSON body.
        assert!(req
            .headers
            .iter()
            .any(|(k, v)| k == "x-api-key" && v == "secret-key"));
        assert!(req
            .headers
            .iter()
            .any(|(k, v)| k == "anthropic-version" && v == ANTHROPIC_VERSION));
        let body_str = String::from_utf8(req.body.clone()).unwrap();
        assert!(!body_str.contains("secret-key"));

        let body = p.transport.sent_body();
        assert_eq!(body["model"], "claude-opus-4-8");
        assert_eq!(body["system"], "be terse");
        assert_eq!(body["max_tokens"], 2048);
    }

    #[tokio::test]
    async fn omits_system_and_tools_when_unset() {
        let p = provider(StubTransport::ok(json!({ "content": [] })));
        p.complete(CompletionRequest::new(
            DEFAULT_MODEL,
            vec![Message::user("hi")],
        ))
        .await
        .unwrap();

        let body = p.transport.sent_body();
        assert!(body.get("system").is_none());
        assert!(body.get("tools").is_none());
        assert_eq!(body["max_tokens"], DEFAULT_MAX_TOKENS);
    }

    #[tokio::test]
    async fn encodes_tools_and_tool_roundtrip() {
        let p = provider(StubTransport::ok(json!({ "content": [] })));
        let tools = vec![ToolSpec {
            name: "lookup".into(),
            description: "look up a position".into(),
            input_schema: json!({ "type": "object", "properties": { "fen": { "type": "string" } } }),
        }];
        let messages = vec![
            Message::user("analyze"),
            Message::Assistant {
                text: Some("checking".into()),
                tool_calls: vec![ToolCall {
                    id: "toolu_1".into(),
                    name: "lookup".into(),
                    input: json!({ "fen": "x" }),
                }],
            },
            Message::ToolResults {
                results: vec![ToolResult {
                    tool_call_id: "toolu_1".into(),
                    content: "found 3 games".into(),
                    is_error: false,
                }],
            },
        ];
        p.complete(CompletionRequest::new(DEFAULT_MODEL, messages).with_tools(tools))
            .await
            .unwrap();

        let body = p.transport.sent_body();
        assert_eq!(body["tools"][0]["name"], "lookup");
        assert_eq!(body["tools"][0]["input_schema"]["type"], "object");

        let msgs = body["messages"].as_array().unwrap();
        // assistant turn with a tool_use block
        assert_eq!(msgs[1]["role"], "assistant");
        assert_eq!(msgs[1]["content"][0]["type"], "text");
        assert_eq!(msgs[1]["content"][1]["type"], "tool_use");
        assert_eq!(msgs[1]["content"][1]["id"], "toolu_1");
        // tool result rides back as a user turn
        assert_eq!(msgs[2]["role"], "user");
        assert_eq!(msgs[2]["content"][0]["type"], "tool_result");
        assert_eq!(msgs[2]["content"][0]["tool_use_id"], "toolu_1");
        assert_eq!(msgs[2]["content"][0]["content"], "found 3 games");
    }

    #[tokio::test]
    async fn maps_429_to_rate_limited() {
        let transport = StubTransport::with_response(HttpResponse {
            status: 429,
            retry_after: Some(Duration::from_secs(7)),
            body: Vec::new(),
        });
        let p = provider(transport);
        let err = p
            .complete(CompletionRequest::new(
                DEFAULT_MODEL,
                vec![Message::user("hi")],
            ))
            .await
            .unwrap_err();
        match err {
            ProviderError::RateLimited { retry_after } => {
                assert_eq!(retry_after, Some(Duration::from_secs(7)));
            }
            other => panic!("expected RateLimited, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn maps_non_success_to_api_error() {
        let transport = StubTransport::with_response(HttpResponse {
            status: 400,
            retry_after: None,
            body: b"{\"error\":\"bad request\"}".to_vec(),
        });
        let p = provider(transport);
        let err = p
            .complete(CompletionRequest::new(
                DEFAULT_MODEL,
                vec![Message::user("hi")],
            ))
            .await
            .unwrap_err();
        match err {
            ProviderError::Api { status, message } => {
                assert_eq!(status, 400);
                assert!(message.contains("bad request"));
            }
            other => panic!("expected Api, got {other:?}"),
        }
    }

    #[test]
    fn provider_metadata() {
        let p = provider(StubTransport::ok(json!({ "content": [] })));
        assert_eq!(p.name(), "anthropic");
        assert_eq!(p.default_model(), DEFAULT_MODEL);
    }

    /// Real network call, gated behind `ANTHROPIC_API_KEY`. Skipped (returns
    /// early) when the key is absent, so the default test run never hits the
    /// network.
    #[tokio::test]
    async fn live_completion_smoke() {
        let Ok(key) = std::env::var("ANTHROPIC_API_KEY") else {
            return;
        };
        let provider = AnthropicProvider::new(key).expect("build provider");
        let resp = provider
            .complete(CompletionRequest::new(
                DEFAULT_MODEL,
                vec![Message::user("Reply with exactly one word: pong")],
            ))
            .await
            .expect("live completion");
        assert!(resp.text.is_some(), "expected text in live response");
    }
}
