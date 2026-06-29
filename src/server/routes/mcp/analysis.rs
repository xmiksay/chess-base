//! Interactive analysis mode (issue #33): the `analyse_position` tool — the
//! tool-using counterpart to the code-orchestrated batch pipeline.
//!
//! It bundles the three grounded sources for a single position into one call so
//! a connected client ("explain this position") gets engine eval, DB stats and
//! feature tags in one round trip and cites tool output rather than inventing
//! lines (ADR-0009: the model synthesises, it never computes). The individual
//! `engine_analyse` / `db_position_report` / `db_reference_games` tools stay
//! available for an agent that wants to drill in further.
//!
//! Every source reuses an existing facade verbatim: the pooled [`EngineService`]
//! (the same one the batch path calls), [`PositionReportService`] (the pre-chewed
//! DB layer, #28), and the pure [`features_of_fen`] extractor. Dispatch / JSON-RPC
//! framing lives in [`super`].
//!
//! [`EngineService`]: crate::engine::EngineService

use std::collections::BTreeMap;

use serde_json::{json, Value};

use super::db_tools::{json_outcome, report_error};
use super::{Tool, ToolOutcome, ToolRegistry};
use crate::features::features_of_fen;
use crate::search::report::PositionReportService;
use crate::server::identity::CurrentUser;
use crate::server::state::AppState;

/// Register the interactive analysis tool into `registry`.
pub fn register(registry: &mut ToolRegistry) {
    registry.register(analyse_position_tool());
}

/// `analyse_position`: one grounded snapshot of a position — engine eval + PV,
/// the pre-chewed DB report, and factual feature tags.
fn analyse_position_tool() -> Tool {
    Tool::new(
        "analyse_position",
        "Explain a single position (FEN) from grounded tool output: it bundles \
         the engine evaluation/principal variation, the synthesized database \
         report (ECO, per-move win/draw/loss, transpositions) and factual feature \
         tags (material, game phase, check/mate, castling rights). Scoped to your \
         databases and the global ones. Base any explanation on these figures — do \
         not invent lines or evaluations. If no engine is configured the `engine` \
         field is null and a note says so; the DB report and features are always \
         present.",
        json!({
            "type": "object",
            "properties": {
                "fen": { "type": "string", "description": "Position to explain, in FEN." },
                "depth": {
                    "type": "integer", "minimum": 1,
                    "description": "Engine search depth in plies (optional; a fixed depth by default)."
                },
                "movetime_ms": {
                    "type": "integer", "minimum": 1,
                    "description": "Engine search time budget in milliseconds (optional)."
                }
            },
            "required": ["fen"]
        }),
        |app, user, args| async move { analyse_position(app, user, args).await },
    )
}

async fn analyse_position(app: AppState, user: CurrentUser, args: Value) -> ToolOutcome {
    let fen = match super::db_tools::fen_arg(&args) {
        Some(fen) => fen,
        None => return ToolOutcome::error("Invalid arguments: missing string field `fen`."),
    };

    // Features double as FEN validation: an illegal FEN fails here, cheaply,
    // before any engine or DB work.
    let features = match features_of_fen(&fen) {
        Ok(features) => features,
        Err(e) => return ToolOutcome::error(format!("invalid FEN: {e}")),
    };

    let report = match PositionReportService::new(app.db.clone())
        .position_report(&user, &fen)
        .await
    {
        Ok(report) => report,
        Err(e) => return report_error(e),
    };

    let mut notes: Vec<String> = Vec::new();
    let engine = engine_analysis(&app, &fen, &args, &mut notes).await;

    json_outcome(&json!({
        "fen": fen,
        "features": features,
        "database": report,
        "engine": engine,
        "notes": notes,
    }))
}

/// Run the pooled engine for the position, or record why it is absent. Returns
/// `None` (and pushes a note) when no engine is configured or the search fails —
/// the explanation is still grounded on DB stats and features.
async fn engine_analysis(
    app: &AppState,
    fen: &str,
    args: &Value,
    notes: &mut Vec<String>,
) -> Option<Value> {
    let Some(service) = app.engine_service.clone() else {
        notes.push(
            "no engine configured: evaluation omitted (start chess-base with --engine)."
                .to_string(),
        );
        return None;
    };

    let limits = super::db_tools::limits_arg(args);

    match service.analyse(fen, &limits, &BTreeMap::new()).await {
        Ok(analysis) => match serde_json::to_value(&analysis) {
            Ok(value) => Some(value),
            Err(_) => {
                notes.push("engine analysis could not be serialised.".to_string());
                None
            }
        },
        Err(e) => {
            notes.push(format!("engine analysis unavailable: {e}"));
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registers_the_analysis_tool() {
        let mut registry = ToolRegistry::new();
        register(&mut registry);
        let list = registry.list();
        let tools = list["tools"].as_array().expect("tools array");
        let tool = tools
            .iter()
            .find(|t| t["name"] == "analyse_position")
            .expect("analyse_position tool");
        assert_eq!(tool["inputSchema"]["required"][0], "fen");
    }
}
