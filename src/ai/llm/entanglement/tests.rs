//! Unit tests for [`StackLlmProvider`] over a scripted stub `Llm` — no network.

use std::sync::{Arc, Mutex};

use entanglement_provider::{
    GenerationParams, Llm, LlmEvent, LlmRequest, LlmStream, Message as StackMessage, MessageRole,
    StopReason, ToolCall as StackToolCall, Usage as StackUsage,
};
use serde_json::json;

use super::*;
use crate::ai::llm::ToolSpec;

/// What the stub backend saw, captured for request-mapping assertions.
#[derive(Default)]
struct Seen {
    system: String,
    model: Option<String>,
    messages: Vec<StackMessage>,
    tool_names: Vec<String>,
    generation: Option<GenerationParams>,
}

/// Stub backend: records the request, replays a scripted event sequence.
struct ScriptLlm {
    script: Arc<dyn Fn() -> Vec<anyhow::Result<LlmEvent>> + Send + Sync>,
    seen: Arc<Mutex<Seen>>,
}

#[async_trait]
impl Llm for ScriptLlm {
    async fn stream(&mut self, req: LlmRequest<'_>) -> anyhow::Result<LlmStream> {
        *self.seen.lock().expect("seen lock") = Seen {
            system: req.system.to_string(),
            model: req.model.map(str::to_string),
            messages: req.messages.to_vec(),
            tool_names: req.tools.iter().map(|t| t.name.clone()).collect(),
            generation: req.generation,
        };
        Ok(Box::pin(tokio_stream::iter((self.script)())))
    }
}

/// A provider over a scripted stub; returns the captured-request handle too.
fn provider_with(
    script: impl Fn() -> Vec<anyhow::Result<LlmEvent>> + Send + Sync + 'static,
) -> (StackLlmProvider, Arc<Mutex<Seen>>) {
    let seen: Arc<Mutex<Seen>> = Arc::default();
    let script: Arc<dyn Fn() -> Vec<anyhow::Result<LlmEvent>> + Send + Sync> = Arc::new(script);
    let factory_seen = seen.clone();
    let resolved = ResolvedModel {
        provider: "stub".to_string(),
        model: "stub-model".to_string(),
        llm_factory: Arc::new(move || {
            Box::new(ScriptLlm {
                script: script.clone(),
                seen: factory_seen.clone(),
            })
        }),
        generation: None,
        context_window: None,
    };
    (StackLlmProvider::from_resolved(resolved), seen)
}

fn finish(usage: StackUsage) -> LlmEvent {
    LlmEvent::Finish {
        stop_reason: Some(StopReason::EndTurn),
        usage,
    }
}

#[tokio::test]
async fn text_only_completion_folds_text_and_maps_usage() {
    let (provider, _) = provider_with(|| {
        vec![
            Ok(LlmEvent::Text("Hello ".into())),
            Ok(LlmEvent::Reasoning("pondering".into())),
            Ok(LlmEvent::Text("world".into())),
            Ok(finish(StackUsage {
                input_tokens: Some(10),
                output_tokens: Some(3),
                cached_input_tokens: Some(5),
                cache_write_tokens: None,
            })),
        ]
    });
    let resp = provider
        .complete(CompletionRequest::new(
            "stub-model",
            vec![Message::user("hi")],
        ))
        .await
        .expect("completion");
    assert_eq!(resp.text.as_deref(), Some("Hello world"));
    assert!(resp.tool_calls.is_empty());
    let usage = resp.usage.expect("usage reported");
    // Total prompt side = uncached + cache reads.
    assert_eq!(usage.input_tokens, 15);
    assert_eq!(usage.output_tokens, 3);
    assert_eq!(provider.name(), "entanglement");
    assert_eq!(provider.default_model(), "stub-model");
}

#[tokio::test]
async fn request_maps_messages_tools_and_max_tokens() {
    let (provider, seen) = provider_with(|| vec![Ok(finish(StackUsage::default()))]);
    let messages = vec![
        Message::user("play e4"),
        Message::Assistant {
            text: Some("adding".to_string()),
            tool_calls: vec![ToolCall {
                id: "c1".into(),
                name: "add_move".into(),
                input: json!({"san": "e4"}),
            }],
        },
        Message::ToolResults {
            results: vec![
                ToolResult {
                    tool_call_id: "c1".into(),
                    content: "ok".into(),
                    is_error: false,
                },
                ToolResult {
                    tool_call_id: "c2".into(),
                    content: "no such node".into(),
                    is_error: true,
                },
            ],
        },
    ];
    let req = CompletionRequest::new("model-x", messages)
        .with_system("be terse")
        .with_tools(vec![ToolSpec {
            name: "add_move".into(),
            description: "add a move".into(),
            input_schema: json!({"type": "object"}),
        }])
        .with_max_tokens(777);
    let resp = provider.complete(req).await.expect("completion");
    assert!(resp.text.is_none(), "empty text folds to None");
    assert!(resp.usage.is_none(), "all-None usage folds to None");

    let seen = seen.lock().expect("seen lock");
    assert_eq!(seen.system, "be terse");
    assert_eq!(seen.model.as_deref(), Some("model-x"));
    assert_eq!(seen.tool_names, vec!["add_move"]);
    assert_eq!(
        seen.generation.expect("generation set").max_output_tokens,
        Some(777)
    );
    // One User + one Assistant + one Tool message per result.
    assert_eq!(seen.messages.len(), 4);
    assert_eq!(seen.messages[0].role, MessageRole::User);
    assert_eq!(seen.messages[1].role, MessageRole::Assistant);
    assert_eq!(seen.messages[1].tool_calls.len(), 1);
    assert_eq!(seen.messages[1].tool_calls[0].input, r#"{"san":"e4"}"#);
    assert_eq!(seen.messages[2].role, MessageRole::Tool);
    assert_eq!(seen.messages[2].tool_call_id.as_deref(), Some("c1"));
    assert_eq!(seen.messages[2].text(), "ok");
    assert_eq!(seen.messages[3].tool_call_id.as_deref(), Some("c2"));
    assert_eq!(seen.messages[3].text(), "[error] no such node");
}

#[tokio::test]
async fn terminal_tool_call_wins_over_its_own_deltas() {
    let (provider, _) = provider_with(|| {
        vec![
            Ok(LlmEvent::ToolCallDelta {
                id: "a".into(),
                name: "add_move".into(),
                delta: r#"{"san""#.into(),
            }),
            Ok(LlmEvent::ToolCallDelta {
                id: "a".into(),
                name: "add_move".into(),
                delta: r#":"e4"}"#.into(),
            }),
            Ok(LlmEvent::ToolCall(StackToolCall::new(
                "a",
                "add_move",
                r#"{"san":"e4"}"#,
            ))),
            Ok(finish(StackUsage::default())),
        ]
    });
    let resp = provider
        .complete(CompletionRequest::new("m", vec![Message::user("go")]))
        .await
        .expect("completion");
    assert_eq!(resp.tool_calls.len(), 1, "no duplicate from the deltas");
    assert_eq!(resp.tool_calls[0].id, "a");
    assert_eq!(resp.tool_calls[0].input, json!({"san": "e4"}));
}

#[tokio::test]
async fn deltas_without_a_terminal_call_assemble_at_end_of_stream() {
    let (provider, _) = provider_with(|| {
        vec![
            Ok(LlmEvent::ToolCallDelta {
                id: "b".into(),
                name: "annotate".into(),
                delta: r#"{"node_id":"#.into(),
            }),
            Ok(LlmEvent::ToolCallDelta {
                id: "b".into(),
                name: "annotate".into(),
                delta: "7}".into(),
            }),
            Ok(finish(StackUsage::default())),
        ]
    });
    let resp = provider
        .complete(CompletionRequest::new("m", vec![Message::user("go")]))
        .await
        .expect("completion");
    assert_eq!(resp.tool_calls.len(), 1);
    assert_eq!(resp.tool_calls[0].name, "annotate");
    assert_eq!(resp.tool_calls[0].input, json!({"node_id": 7}));
}

#[tokio::test]
async fn empty_tool_arguments_parse_as_an_empty_object() {
    let (provider, _) = provider_with(|| {
        vec![
            Ok(LlmEvent::ToolCall(StackToolCall::new("c", "export", ""))),
            Ok(finish(StackUsage::default())),
        ]
    });
    let resp = provider
        .complete(CompletionRequest::new("m", vec![Message::user("go")]))
        .await
        .expect("completion");
    assert_eq!(resp.tool_calls[0].input, json!({}));
}

#[tokio::test]
async fn invalid_tool_input_is_a_protocol_error() {
    let (provider, _) = provider_with(|| {
        vec![Ok(LlmEvent::ToolCall(StackToolCall::new(
            "c",
            "add_move",
            "{not json",
        )))]
    });
    let err = provider
        .complete(CompletionRequest::new("m", vec![Message::user("go")]))
        .await
        .expect_err("invalid JSON must fail");
    assert!(matches!(err, ProviderError::Protocol(_)), "got {err:?}");
}

#[tokio::test]
async fn mid_stream_error_is_a_transport_error() {
    let (provider, _) = provider_with(|| {
        vec![
            Ok(LlmEvent::Text("partial".into())),
            Err(anyhow::anyhow!("connection reset")),
        ]
    });
    let err = provider
        .complete(CompletionRequest::new("m", vec![Message::user("go")]))
        .await
        .expect_err("stream error must fail");
    match err {
        ProviderError::Transport(msg) => assert!(msg.contains("connection reset"), "{msg}"),
        other => panic!("expected Transport, got {other:?}"),
    }
}

#[test]
fn resolver_error_maps_to_a_config_error() {
    let resolver: ModelResolver = Arc::new(|_, _, _| Err("no LLM provider configured".to_string()));
    let Err(err) = StackLlmProvider::resolve(&resolver, None, "anthropic", "claude-x") else {
        panic!("resolver failure must surface");
    };
    match err {
        ProviderError::Config(msg) => assert_eq!(msg, "no LLM provider configured"),
        other => panic!("expected Config, got {other:?}"),
    }
}
