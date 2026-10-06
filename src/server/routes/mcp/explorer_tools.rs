//! MCP twin of `GET /api/explorer/masters` (ADR-0053): Lichess Masters stats for
//! a position, read-only. Off the anonymous allowlist (signed-in only, like the
//! HTTP route); the import route stays HTTP-only (see `symmetry.rs`).

use serde_json::{json, Value};

use super::db_tools::{fen_arg, json_outcome};
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
        "Lichess Masters database stats for a position (FEN): total games and \
         White/draw/Black counts, the ECO opening, per-move continuations (count, \
         W/D/B, average rating) and notable top games (players, ratings, year, \
         Lichess id). A remote over-the-board reference (2200+ classical games), \
         distinct from your own databases (`db_position_report`). Optional \
         `since`/`until` restrict to a year window.",
        json!({
            "type": "object",
            "properties": {
                "fen": { "type": "string", "description": "Position to report on, in FEN." },
                "since": { "type": "integer", "description": "Only games from this year on." },
                "until": { "type": "integer", "description": "Only games up to this year." }
            },
            "required": ["fen"]
        }),
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
    let Some(fen) = fen_arg(&args) else {
        return ToolOutcome::error("Invalid arguments: missing string field `fen`.");
    };
    let year = |key: &str| {
        args.get(key)
            .and_then(Value::as_u64)
            .and_then(|y| u16::try_from(y).ok())
    };
    let query = MastersQuery {
        fen,
        since: year("since"),
        until: year("until"),
    };
    match client.report(&query).await {
        Ok(report) => json_outcome(&report),
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
