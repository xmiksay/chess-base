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

use super::db_tools::{json_outcome, opt_bounded_u64, report_error};
use super::{Tool, ToolOutcome, ToolRegistry};
use crate::engine::{DEFAULT_DEPTH, MAX_DEPTH, MAX_MOVETIME_MS};
use crate::features::features_of_fen;
use crate::games::export::annotated_tree;
use crate::ingest::parse_pgn;
use crate::pgn_tree::pgn;
use crate::position::STARTPOS_FEN;
use crate::review::{review_game, ReviewError};
use crate::search::position::PositionFilter;
use crate::search::report::PositionReportService;
use crate::server::identity::CurrentUser;
use crate::server::state::AppState;

/// Register the interactive analysis tools into `registry`.
pub fn register(registry: &mut ToolRegistry) {
    registry.register(analyse_position_tool());
    registry.register(analyse_game_tool());
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
                    "type": "integer", "minimum": 1, "maximum": MAX_DEPTH,
                    "description": format!(
                        "Engine search depth in plies (optional; defaults to depth \
                         {DEFAULT_DEPTH}, the same default `engine_analyse` uses); capped server-side."
                    )
                },
                "movetime_ms": {
                    "type": "integer", "minimum": 1, "maximum": MAX_MOVETIME_MS,
                    "description": "Engine search time budget in milliseconds (optional); capped server-side."
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
        .position_report(&user, &fen, &PositionFilter::default())
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

    let limits = match super::db_tools::limits_arg(args) {
        Ok(limits) => limits,
        Err(msg) => {
            notes.push(msg);
            return None;
        }
    };

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

/// `analyse_game`: walk the engine over a whole game (from PGN) and return the
/// per-ply review — the #119 facts — plus the annotated movetext.
fn analyse_game_tool() -> Tool {
    Tool::new(
        "analyse_game",
        "Review a whole game given as PGN: the engine walks every ply and returns, \
         per move, the evaluation (White's perspective, centipawns or mate), the \
         engine's best move, the played move's rank, a quality classification \
         (best/great/good/inaccuracy/mistake/blunder) and a terse factual note, \
         plus a per-side accuracy summary. Also returns the same game as annotated \
         PGN movetext (`[%eval]` + NAGs + notes) for re-import. Ground any \
         narrative in these figures — do not invent evaluations. Requires an \
         engine configured.",
        json!({
            "type": "object",
            "properties": {
                "pgn": { "type": "string", "description": "The game to review, as PGN movetext (first game)." },
                "depth": {
                    "type": "integer", "minimum": 1, "maximum": MAX_DEPTH,
                    "description": format!(
                        "Per-ply engine search depth in plies (optional; defaults to depth \
                         {DEFAULT_DEPTH}); capped server-side."
                    )
                }
            },
            "required": ["pgn"]
        }),
        |app, _user, args| async move { analyse_game(app, args).await },
    )
}

async fn analyse_game(app: AppState, args: Value) -> ToolOutcome {
    let Some(engine) = app.engine_service.clone() else {
        return ToolOutcome::error(
            "No engine configured: start chess-base with --engine / CHESS_BASE_ENGINE.",
        );
    };
    let Some(pgn) = args
        .get("pgn")
        .and_then(Value::as_str)
        .filter(|p| !p.trim().is_empty())
    else {
        return ToolOutcome::error("Invalid arguments: missing string field `pgn`.");
    };
    let depth = match opt_bounded_u64(&args, "depth", MAX_DEPTH as u64) {
        Ok(depth) => depth.map(|d| d as u32).unwrap_or(DEFAULT_DEPTH),
        Err(msg) => return ToolOutcome::error(msg),
    };

    let parsed = match parse_pgn(pgn) {
        Ok(parsed) => parsed,
        Err(e) => return ToolOutcome::error(format!("could not parse PGN: {e}")),
    };
    if parsed.mainline.is_empty() {
        return ToolOutcome::error("the PGN has no moves to analyse");
    }
    let start_fen = parsed.headers.start_fen.as_deref().unwrap_or(STARTPOS_FEN);
    let variant = parsed.headers.variant.as_deref().unwrap_or("standard");

    let review = match review_game(&engine, start_fen, variant, &parsed.mainline, depth).await {
        Ok(review) => review,
        Err(e) => return review_error(e),
    };
    // Reuse the single shared serializer (#120) for the annotated movetext.
    let tree = annotated_tree(&parsed.mainline, &review);
    let movetext = match pgn::to_pgn(&tree) {
        Ok(text) => text,
        Err(e) => return ToolOutcome::error(format!("failed to serialise PGN: {e}")),
    };

    json_outcome(&json!({
        "start_fen": review.start_fen,
        "moves": review.moves,
        "summary": review.summary,
        "pgn": movetext,
    }))
}

/// Map a [`ReviewError`] onto a tool outcome: a bad game carries its safe message;
/// an engine failure is masked so no engine internals leak.
fn review_error(error: ReviewError) -> ToolOutcome {
    match error {
        ReviewError::BadGame(msg) => ToolOutcome::error(format!("could not review game: {msg}")),
        ReviewError::Engine(_) => ToolOutcome::error("engine analysis failed"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn registry() -> ToolRegistry {
        let mut registry = ToolRegistry::new();
        register(&mut registry);
        registry
    }

    #[test]
    fn registers_the_analysis_tools() {
        let list = registry().list();
        let tools = list["tools"].as_array().expect("tools array");
        let tool = tools
            .iter()
            .find(|t| t["name"] == "analyse_position")
            .expect("analyse_position tool");
        assert_eq!(tool["inputSchema"]["required"][0], "fen");
        let game = tools
            .iter()
            .find(|t| t["name"] == "analyse_game")
            .expect("analyse_game tool");
        assert_eq!(game["inputSchema"]["required"][0], "pgn");
    }
}
