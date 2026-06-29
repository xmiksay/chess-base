//! Tests for [`super`] (Anthropic Messages API client). Split out to keep the
//! module under the project's 500-line file cap.

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
