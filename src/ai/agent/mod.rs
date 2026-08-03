//! Embedded entanglement agent engine (#198).
//!
//! Holds the pieces that survived the hand-rolled assistant (moved verbatim
//! from the deleted `ai::assistant`): the system prompt steering the study
//! agent and the approval gate over mutating MCP tools (ADR-0025).
//! [`provider_store`] implements entanglement's `UserProviderStore` seam over
//! the `llm_providers` table; [`policy`] its `PermissionResolver`/`GrantStore`
//! seams over `agent_grants` (m0008); [`tools`] bridges the MCP registry 1:1
//! into the engine's tool vocabulary. Step 4 adds the running stack:
//! [`persistence`] (the `agent_events` `RecordSink` + log reads), [`sessions`]
//! (conversation CRUD/lifecycle over `agent_sessions`) and [`engine`] (the
//! process-wide `Holly` bootstrap on [`AppState`]).
//!
//! [`AppState`]: crate::server::state::AppState

pub mod engine;
pub mod persistence;
pub mod policy;
pub mod provider_store;
pub mod sessions;
pub mod tools;

pub use engine::AgentEngine;
pub use persistence::{load_records, DbRecordSink};
pub use policy::{base_profile, AgentPolicy};
pub use provider_store::{AgentProviderStore, DEFAULT_PIN, HOUSE_USER};
pub use sessions::{AgentSessionSummary, OpenResult, SessionError, SessionService};
pub use tools::{bridge_registry, BridgedTool};

/// The tools whose effects mutate the caller's data and therefore require an
/// explicit approval before the loop runs them. Everything else (engine/database
/// reads, exports) runs automatically. Matched by the registered MCP tool names.
pub const GATED_TOOLS: &[&str] = &[
    "study_create",
    "study_import_pgn",
    "study_add_move",
    "study_annotate",
    "study_set_folder",
    "study_set_shapes",
    "study_promote_node",
    "study_reorder_node",
    "study_merge_games",
    "study_merge_danger",
    "study_analyse",
    "study_clear_shapes",
    "game_save_as_study",
    "game_delete",
    "folder_create",
    "folder_update",
    "folder_delete",
    "import_pgn",
    "import_sync",
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gating_marks_only_mutating_tools() {
        assert!(requires_approval("study_create"));
        assert!(requires_approval("study_annotate"));
        assert!(requires_approval("study_clear_shapes"));
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
    fn gated_list_covers_the_mutating_surface() {
        assert_eq!(GATED_TOOLS.len(), 19);
    }
}
