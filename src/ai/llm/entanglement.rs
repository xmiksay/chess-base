//! [`StackLlmProvider`] — the batch-LLM adapter over the embedded entanglement
//! provider stack (#198, step 6).
//!
//! The study-generation paths (generate / annotate / danger maps) keep the old
//! one-shot [`LlmProvider`] contract; this adapter resolves a `(provider,
//! model)` pair through entanglement's [`ModelResolver`] (the same per-user
//! resolution the agent engine uses) and fulfils `complete()` by building a
//! fresh backend from the resolved factory and draining its event stream to a
//! single [`CompletionResponse`]. Reasoning and provider-native content blocks
//! are not part of the old contract and are dropped.

use async_trait::async_trait;
use entanglement_provider::{
    LlmEvent, LlmRequest, LlmStream, Message as StackMessage, ModelResolver, ResolvedModel,
    ToolCall as StackToolCall, ToolSpec as StackToolSpec, Usage as StackUsage, UserId,
};
use serde_json::{Map, Value};
use tokio_stream::StreamExt;

use super::{
    CompletionRequest, CompletionResponse, LlmProvider, Message, ProviderError, ToolCall,
    ToolResult, Usage,
};

/// One resolved per-user model binding, usable as a [`LlmProvider`]. Cheap to
/// construct per request — resolution is a pure catalog lookup (no network) and
/// the factory builds a backend over the already-warm shared HTTP client.
pub struct StackLlmProvider {
    resolved: ResolvedModel,
}

impl StackLlmProvider {
    /// Resolve `(provider, model)` for `user` (`None` ⇒ the house context, see
    /// `AgentProviderStore::build_resolver`). The resolver's own message is the
    /// error surface — it never leaks keys or internals.
    pub fn resolve(
        resolver: &ModelResolver,
        user: Option<&UserId>,
        provider: &str,
        model: &str,
    ) -> Result<Self, ProviderError> {
        let resolved = resolver(user, provider, model).map_err(ProviderError::Config)?;
        Ok(Self::from_resolved(resolved))
    }

    /// Wrap an already-resolved model — the hermetic test seam; production
    /// callers go through [`Self::resolve`].
    pub fn from_resolved(resolved: ResolvedModel) -> Self {
        Self { resolved }
    }
}

#[async_trait]
impl LlmProvider for StackLlmProvider {
    async fn complete(&self, req: CompletionRequest) -> Result<CompletionResponse, ProviderError> {
        let messages = to_stack_messages(&req.messages);
        let tools: Vec<StackToolSpec> = req
            .tools
            .iter()
            .map(|t| {
                StackToolSpec::with_schema(
                    t.name.clone(),
                    t.description.clone(),
                    t.input_schema.clone(),
                )
            })
            .collect();
        // The resolved model's catalog knobs are the base; the request's
        // max_tokens is the one knob the old contract carries.
        let mut generation = self.resolved.generation.unwrap_or_default();
        generation.max_output_tokens = Some(req.max_tokens);
        let model = if req.model.is_empty() {
            self.resolved.model.as_str()
        } else {
            req.model.as_str()
        };

        let mut llm = (self.resolved.llm_factory)();
        let stream = llm
            .stream(LlmRequest {
                system: &req.system,
                model: Some(model),
                messages: &messages,
                tools: &tools,
                generation: Some(generation),
            })
            .await
            .map_err(|e| ProviderError::Transport(format!("{e:#}")))?;
        drain(stream).await
    }

    fn name(&self) -> &'static str {
        "entanglement"
    }

    fn default_model(&self) -> &str {
        &self.resolved.model
    }
}

/// Flatten the batch contract's turns into entanglement's message list: a
/// `ToolResults` turn fans out to one `Tool`-role message per result (the
/// per-wire converters require `tool_call_id` per message). A result's
/// `is_error` has no entanglement field, so it is folded into the text.
fn to_stack_messages(messages: &[Message]) -> Vec<StackMessage> {
    let mut out = Vec::new();
    for msg in messages {
        match msg {
            Message::User { text } => out.push(StackMessage::user(text.clone())),
            Message::Assistant { text, tool_calls } => out.push(StackMessage::assistant(
                text.clone().unwrap_or_default(),
                tool_calls.iter().map(to_stack_call).collect(),
            )),
            Message::ToolResults { results } => {
                for r in results {
                    out.push(StackMessage::tool(r.tool_call_id.clone(), result_text(r)));
                }
            }
        }
    }
    out
}

fn result_text(r: &ToolResult) -> String {
    if r.is_error {
        format!("[error] {}", r.content)
    } else {
        r.content.clone()
    }
}

/// Entanglement carries tool-call input as the raw JSON argument string; the
/// batch contract carries it parsed.
fn to_stack_call(c: &ToolCall) -> StackToolCall {
    StackToolCall::new(c.id.clone(), c.name.clone(), c.input.to_string())
}

/// Drain a completed stream into the one-shot response. Tool calls normally
/// arrive fully assembled as terminal [`LlmEvent::ToolCall`]s (the deltas are
/// additive live-display fragments — entanglement's own turn loop ignores
/// them); the delta accumulator is a fallback finalized at end-of-stream only
/// for a call whose terminal event never arrived.
async fn drain(mut stream: LlmStream) -> Result<CompletionResponse, ProviderError> {
    let mut text = String::new();
    let mut calls: Vec<ToolCall> = Vec::new();
    // (id, name, accumulated JSON args) in first-fragment order.
    let mut pending: Vec<(String, String, String)> = Vec::new();
    let mut usage = None;
    while let Some(ev) = stream.next().await {
        match ev.map_err(|e| ProviderError::Transport(format!("{e:#}")))? {
            LlmEvent::Text(t) => text.push_str(&t),
            LlmEvent::Reasoning(_) | LlmEvent::ContentBlock(_) => {}
            LlmEvent::ToolCallDelta { id, name, delta } => {
                match pending.iter_mut().find(|(pid, _, _)| *pid == id) {
                    Some((_, _, args)) => args.push_str(&delta),
                    None => pending.push((id, name, delta)),
                }
            }
            LlmEvent::ToolCall(call) => {
                pending.retain(|(pid, _, _)| *pid != call.id);
                calls.push(parse_call(&call.id, &call.name, &call.input)?);
            }
            LlmEvent::Finish { usage: u, .. } => usage = map_usage(u),
        }
    }
    for (id, name, args) in pending {
        calls.push(parse_call(&id, &name, &args)?);
    }
    Ok(CompletionResponse {
        text: (!text.is_empty()).then_some(text),
        tool_calls: calls,
        usage,
    })
}

/// Parse a call's raw JSON argument text back into the contract's `Value`
/// input. An empty argument string means a no-argument call (`{}`).
fn parse_call(id: &str, name: &str, input: &str) -> Result<ToolCall, ProviderError> {
    let input = if input.trim().is_empty() {
        Value::Object(Map::new())
    } else {
        serde_json::from_str(input).map_err(|e| {
            ProviderError::Protocol(format!(
                "tool call `{name}` carried invalid JSON input: {e}"
            ))
        })?
    };
    Ok(ToolCall {
        id: id.to_string(),
        name: name.to_string(),
        input,
    })
}

/// Fold entanglement's normalized usage onto the contract's two counters:
/// `input_tokens` is the *total* prompt side (uncached + cache reads), matching
/// what the old Anthropic client reported. All-`None` usage maps to `None`.
fn map_usage(u: StackUsage) -> Option<Usage> {
    if u == StackUsage::default() {
        return None;
    }
    let input = u.input_tokens.unwrap_or(0) + u.cached_input_tokens.unwrap_or(0);
    Some(Usage {
        input_tokens: i64::try_from(input).unwrap_or(i64::MAX),
        output_tokens: i64::try_from(u.output_tokens.unwrap_or(0)).unwrap_or(i64::MAX),
    })
}

#[cfg(test)]
mod tests;
