//! MCP twin of `GET /api/explorer/masters` (ADR-0053): Lichess Masters stats for
//! a position, read-only. Off the anonymous allowlist (signed-in only, like the
//! HTTP route). Importing a top game goes through `import_lichess_game`
//! (ADR-0057), not a Masters-specific tool.

use serde_json::{json, Value};

use super::db_tools::json_outcome;
use super::position_arg::{position_schema, require_position, with_fen};
use super::{Tool, ToolOutcome, ToolRegistry};
use crate::explorer::{MastersError, MastersQuery};
use crate::server::identity::CurrentUser;
use crate::server::state::AppState;

pub fn register(registry: &mut ToolRegistry) {
    registry.register(masters_position_report_tool());
}

fn masters_position_report_tool() -> Tool {
    Tool::new(
        "masters_position_report",
        "Lichess Masters database stats for a position: total games and \
         White/draw/Black counts, the ECO opening, per-move continuations (`moves`: \
         SAN, count, W/D/B, average rating) and notable top games (players, ratings, \
         year, Lichess id). A remote over-the-board reference (2200+ classical \
         games), distinct from your own databases (`db_position_report`). Optional \
         `since`/`until` restrict to a year window.\n\
         Give the position as `moves` (SAN from the start), never a hand-written \
         FEN; the reply echoes the resolved `fen`. `total: 0` means no master game \
         reached this exact position (out of book, or the wrong move order): it is \
         not an outage, and the feed has no depth limit.\n\
         Building an opening study from Masters: start at the opening's moves, read \
         `moves`, keep the few continuations that carry most of `total` (e.g. ≥5% \
         and ≥50 games), then call again with that SAN appended — one call per node, \
         so budget breadth × depth (a 20-line × 10-move tree is hundreds of calls; \
         branch only where it matters and follow the main line deeper). Write the \
         result as one PGN with RAV variations, putting each branch's stats in its \
         comment (\"1,234 games, White 38% / draw 41% / Black 21%\") and naming \
         model games from `top_games` (players, year); `import_lichess_game` pulls one \
         into a database by its id. Persist with \
         `study_import_pgn` into a `database_id` from `list_databases`, then \
         `study_analyse` for engine evals and move-quality NAGs, then \
         `study_annotate` for your prose. Quote the figures, never invent them.",
        position_schema(
            "Position to report on",
            json!({
                "since": { "type": "integer", "description": "Only games from this year on." },
                "until": { "type": "integer", "description": "Only games up to this year." }
            }),
        ),
        |app, user, args| async move { masters_position_report(app, user, args).await },
    )
}

async fn masters_position_report(app: AppState, user: CurrentUser, args: Value) -> ToolOutcome {
    if user.public {
        return ToolOutcome::error("masters_position_report requires signing in.");
    }
    let Some(client) = app.masters.as_deref() else {
        return ToolOutcome::error("The Masters explorer is not configured on this server.");
    };
    let fen = match require_position(&args) {
        Ok(fen) => fen,
        Err(msg) => return ToolOutcome::error(msg),
    };
    let year = |key: &str| {
        args.get(key)
            .and_then(Value::as_u64)
            .and_then(|y| u16::try_from(y).ok())
    };
    let query = MastersQuery {
        fen: fen.clone(),
        since: year("since"),
        until: year("until"),
    };
    match client.report(&query).await {
        Ok(report) => json_outcome(&with_fen(&report, &fen)),
        Err(MastersError::Upstream(e)) => {
            tracing::warn!(error = %format!("{e:#}"), "masters_position_report upstream failure");
            ToolOutcome::error(MastersError::Upstream(e).to_string())
        }
        Err(e) => ToolOutcome::error(e.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::super::tools::default_registry;

    #[test]
    fn masters_position_report_is_registered() {
        let list = default_registry().list();
        let tools = list["tools"].as_array().expect("tools array");
        assert!(tools.iter().any(|t| t["name"] == "masters_position_report"));
    }
}
