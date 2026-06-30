//! Embedded Claude study assistant (issue #20, Direction B): an in-app chat that
//! drives an agent loop over the **same** tool surface the `/mcp` transport
//! exposes (engine / database / study tools) — no second implementation. The loop
//! has a hard iteration cap and gates every *mutating* tool behind explicit user
//! approval.
//!
//! This module holds the transport-agnostic pieces: the persisted [`store`], the
//! agent-loop [`service`], and the pure view/gating helpers below (unit-tested
//! here). The HTTP surface is `server::routes::assistant`.

pub mod service;
pub mod store;

use serde::Serialize;
use serde_json::Value;

use crate::ai::llm::{Message, ToolCall, ToolResult, ToolSpec};
use crate::server::routes::mcp::ToolRegistry;

pub use service::AssistantService;
pub use store::{AssistantError, AssistantStore};

/// Hard cap on agent-loop tool rounds per user message. Each round is one batch
/// of executed tool calls; once reached the loop stops and tells the user. Keeps
/// a runaway model from looping the engine/LLM indefinitely (and is surfaced to
/// the SPA so the cap is *visible*, per the acceptance criteria).
pub const MAX_ITERATIONS: usize = 8;

/// Per-response output cap for the interactive loop. Smaller than the batch
/// annotation default — chat turns are short and this keeps latency down.
pub const MAX_TOKENS: u32 = 4_096;

/// Default session title before the first message names it.
pub const DEFAULT_TITLE: &str = "New chat";

/// The tools whose effects mutate the caller's data and therefore require an
/// explicit approval before the loop runs them. Everything else (engine/database
/// reads, exports) runs automatically. Matched by the registered MCP tool names.
const GATED_TOOLS: &[&str] = &[
    "study_create",
    "study_import_pgn",
    "study_add_move",
    "study_annotate",
];

/// Does running this tool need explicit user approval? (mutating tools do).
pub fn requires_approval(tool_name: &str) -> bool {
    GATED_TOOLS.contains(&tool_name)
}

/// The system prompt steering the assistant: a grounded chess study-builder that
/// cites tool output and leans on the study tools to persist its work.
pub const SYSTEM_PROMPT: &str = "\
You are the chess-base study assistant, embedded in a self-hosted ChessBase \
replacement. You help the user analyse positions and build annotated studies \
(opening repertoires, model games, tactical sets).

Work through the provided tools rather than from memory:
- Discover the user's collections with `list_databases` to get a `database_id`.
- Ground every evaluation, best move and variation in `engine_analyse` / \
  `analyse_position` and the database tools — never assert an eval or line you \
  have not verified with a tool.
- To build an opening study, scaffold it with the preprocessing tools — \
  `opening_tree` for the pruned variation skeleton, `danger_map` for a \
  repertoire's traps and only-moves, `position_concepts` for the pawn structure \
  — then write the annotations yourself: those tools return data, not prose. \
  For a large skeleton, pass `save_as` to `opening_tree` / `danger_map` to persist \
  the whole tree into a study in one call (you get back a `study_id`, not the tree), \
  then layer the prose with `study_annotate`.
- Build and edit studies with the study tools (`study_create`, `study_add_move`, \
  `study_annotate`, `study_import_pgn`).

When you write study text, embed positions with `<fen>FEN</fen>` and games with \
`<pgn move=\"N\">moves</pgn>`. The tools that change the user's data require their \
approval before they run, so explain what you intend to do, then call the tool. \
Be concise.";

/// Note appended (as a final assistant turn) when the loop hits [`MAX_ITERATIONS`]
/// so the transcript records why it stopped.
pub const CAP_NOTE: &str = "I've reached the tool-use limit for this turn. Send \
another message if you'd like me to continue.";

// --- View DTOs (serialized to the SPA) ----------------------------------

/// A model-requested tool call, with whether it needs approval, for the SPA.
#[derive(Debug, Clone, Serialize)]
pub struct ToolCallView {
    pub id: String,
    pub name: String,
    pub input: Value,
    pub requires_approval: bool,
}

impl From<&ToolCall> for ToolCallView {
    fn from(c: &ToolCall) -> Self {
        Self {
            id: c.id.clone(),
            name: c.name.clone(),
            input: c.input.clone(),
            requires_approval: requires_approval(&c.name),
        }
    }
}

/// A tool result fed back into the loop, for the SPA transcript.
#[derive(Debug, Clone, Serialize)]
pub struct ToolResultView {
    pub tool_call_id: String,
    pub content: String,
    pub is_error: bool,
}

impl From<&ToolResult> for ToolResultView {
    fn from(r: &ToolResult) -> Self {
        Self {
            tool_call_id: r.tool_call_id.clone(),
            content: r.content.clone(),
            is_error: r.is_error,
        }
    }
}

/// One transcript turn flattened for the SPA: `role` plus whichever of text /
/// tool-calls / tool-results applies.
#[derive(Debug, Clone, Serialize)]
pub struct MessageView {
    pub role: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ToolCallView>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tool_results: Vec<ToolResultView>,
}

impl From<&Message> for MessageView {
    fn from(m: &Message) -> Self {
        match m {
            Message::User { text } => Self {
                role: "user",
                text: Some(text.clone()),
                tool_calls: Vec::new(),
                tool_results: Vec::new(),
            },
            Message::Assistant { text, tool_calls } => Self {
                role: "assistant",
                text: text.clone(),
                tool_calls: tool_calls.iter().map(ToolCallView::from).collect(),
                tool_results: Vec::new(),
            },
            Message::ToolResults { results } => Self {
                role: "tool_results",
                text: None,
                tool_calls: Vec::new(),
                tool_results: results.iter().map(ToolResultView::from).collect(),
            },
        }
    }
}

/// A full session with its transcript and loop state, returned after every
/// create / post / respond and by `GET …/{id}`.
#[derive(Debug, Clone, Serialize)]
pub struct SessionView {
    pub id: i32,
    pub title: String,
    pub model: String,
    pub messages: Vec<MessageView>,
    /// Mutating calls awaiting the user's approve/deny decision (empty unless
    /// `awaiting_approval`).
    pub pending_approvals: Vec<ToolCallView>,
    pub awaiting_approval: bool,
    /// Tool rounds run since the last user message, and the cap they stop at.
    pub iterations: usize,
    pub iteration_cap: usize,
}

/// Lightweight session row for the sidebar list (no transcript).
#[derive(Debug, Clone, Serialize)]
pub struct SessionSummary {
    pub id: i32,
    pub title: String,
    pub model: String,
}

impl From<crate::db::entities::assistant_sessions::Model> for SessionSummary {
    fn from(m: crate::db::entities::assistant_sessions::Model) -> Self {
        Self {
            id: m.id,
            title: m.title,
            model: m.model,
        }
    }
}

// --- Pure loop helpers ---------------------------------------------------

/// Build the LLM tool specs from the in-process tool registry (the same tools the
/// `/mcp` transport serves) so the assistant and MCP share one surface.
pub fn tool_specs(registry: &ToolRegistry) -> Vec<ToolSpec> {
    registry
        .tools()
        .iter()
        .map(|t| ToolSpec {
            name: t.name.to_string(),
            description: t.description.to_string(),
            input_schema: t.input_schema.clone(),
        })
        .collect()
}

/// The unanswered tool calls of the latest assistant turn, if the transcript ends
/// on one. Results always immediately follow their assistant turn, so a trailing
/// assistant-with-tool-calls means the loop is paused awaiting them.
pub fn last_unanswered_calls(messages: &[Message]) -> Option<Vec<ToolCall>> {
    match messages.last() {
        Some(Message::Assistant { tool_calls, .. }) if !tool_calls.is_empty() => {
            Some(tool_calls.clone())
        }
        _ => None,
    }
}

/// The gated subset of the pending tool calls, as views for the SPA.
pub fn pending_approvals(messages: &[Message]) -> Vec<ToolCallView> {
    last_unanswered_calls(messages)
        .into_iter()
        .flatten()
        .filter(|c| requires_approval(&c.name))
        .map(|c| ToolCallView::from(&c))
        .collect()
}

/// Assemble the full [`SessionView`] from a session row and its transcript.
pub fn build_view(
    session: &crate::db::entities::assistant_sessions::Model,
    messages: &[Message],
) -> SessionView {
    SessionView {
        id: session.id,
        title: session.title.clone(),
        model: session.model.clone(),
        messages: messages.iter().map(MessageView::from).collect(),
        pending_approvals: pending_approvals(messages),
        awaiting_approval: last_unanswered_calls(messages).is_some(),
        iterations: iterations_since_user(messages),
        iteration_cap: MAX_ITERATIONS,
    }
}

/// Tool rounds (executed `ToolResults` batches) since the last user message —
/// the value the [`MAX_ITERATIONS`] cap is checked against.
pub fn iterations_since_user(messages: &[Message]) -> usize {
    let mut count = 0;
    for m in messages.iter().rev() {
        match m {
            Message::User { .. } => break,
            Message::ToolResults { .. } => count += 1,
            Message::Assistant { .. } => {}
        }
    }
    count
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn call(name: &str) -> ToolCall {
        ToolCall {
            id: format!("c_{name}"),
            name: name.to_string(),
            input: json!({}),
        }
    }

    #[test]
    fn gating_marks_only_mutating_tools() {
        assert!(requires_approval("study_create"));
        assert!(requires_approval("study_annotate"));
        // The preprocessing tools return data, not mutations — they run without
        // approval, like the engine/DB reads (ADR-0027).
        assert!(!requires_approval("opening_tree"));
        assert!(!requires_approval("danger_map"));
        assert!(!requires_approval("position_concepts"));
        assert!(!requires_approval("engine_analyse"));
        assert!(!requires_approval("list_databases"));
        assert!(!requires_approval("study_get"));
    }

    #[test]
    fn pending_approvals_returns_only_gated_calls_of_the_last_turn() {
        let messages = vec![
            Message::user("hi"),
            Message::Assistant {
                text: Some("on it".to_string()),
                tool_calls: vec![call("engine_analyse"), call("study_create")],
            },
        ];
        let pending = pending_approvals(&messages);
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].name, "study_create");
        assert!(pending[0].requires_approval);
    }

    #[test]
    fn no_pending_when_last_turn_is_answered() {
        let messages = vec![
            Message::user("hi"),
            Message::Assistant {
                text: None,
                tool_calls: vec![call("study_create")],
            },
            Message::ToolResults {
                results: vec![ToolResult {
                    tool_call_id: "c_study_create".to_string(),
                    content: "{}".to_string(),
                    is_error: false,
                }],
            },
        ];
        assert!(last_unanswered_calls(&messages).is_none());
        assert!(pending_approvals(&messages).is_empty());
    }

    #[test]
    fn iterations_count_resets_at_the_last_user_message() {
        let messages = vec![
            Message::user("first"),
            Message::ToolResults { results: vec![] },
            Message::user("second"),
            Message::Assistant {
                text: None,
                tool_calls: vec![call("engine_analyse")],
            },
            Message::ToolResults { results: vec![] },
        ];
        // Only the round after the second user message counts.
        assert_eq!(iterations_since_user(&messages), 1);
    }
}
