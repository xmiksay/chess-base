//! MCP database tools (issue #28): the *pre-chewed* query surface. Each tool
//! returns synthesized data — ECO, per-move win/draw/loss + frequency/score,
//! transpositions, reference games — from [`PositionReportService`], scoped to
//! the authenticated caller (ADR-0016). The model consumes conclusions; it never
//! computes them (ADR-0009). Dispatch/JSON-RPC framing lives in [`super::mcp`].

use serde::Serialize;
use serde_json::{json, Value};

use super::mcp::{Tool, ToolOutcome, ToolRegistry};
use crate::search::report::PositionReportService;
use crate::search::SearchError;
use crate::server::identity::CurrentUser;
use crate::server::state::AppState;

/// Reference games returned when the caller gives no explicit `limit`.
const DEFAULT_REFERENCE_LIMIT: u64 = 20;

/// Register the pre-chewed DB query tools into `registry`.
pub fn register(registry: &mut ToolRegistry) {
    registry.register(position_report_tool());
    registry.register(reference_games_tool());
}

/// `db_position_report`: ECO + per-move stats (with frequency/score) + transpositions.
fn position_report_tool() -> Tool {
    Tool::new(
        "db_position_report",
        "Synthesized database report for a position (FEN): its ECO code+name, the \
         per-move statistics — count, win/draw/loss (white/draws/black), frequency \
         and White's score — and the transpositions (distinct move orders that \
         reach it). Scoped to your databases and the global ones. Use it as ground \
         truth for opening/structure facts; do not recompute the figures.",
        json!({
            "type": "object",
            "properties": {
                "fen": { "type": "string", "description": "Position to report on, in FEN." }
            },
            "required": ["fen"]
        }),
        |app, user, args| async move { position_report(app, user, args).await },
    )
}

async fn position_report(app: AppState, user: CurrentUser, args: Value) -> ToolOutcome {
    let Some(fen) = fen_arg(&args) else {
        return ToolOutcome::error("Invalid arguments: missing string field `fen`.");
    };
    let service = PositionReportService::new(app.db.clone());
    match service.position_report(&user, &fen).await {
        Ok(report) => json_outcome(&report),
        Err(e) => report_error(e),
    }
}

/// `db_reference_games`: scoped reference/typical games reaching a position.
fn reference_games_tool() -> Tool {
    Tool::new(
        "db_reference_games",
        "Reference / typical games reaching a position (FEN), scoped to your \
         databases and the global ones. Returns game headers (players, result, \
         ECO, Elo) oldest-first, capped by `limit` (default 20). Use these as \
         concrete examples of a line or structure.",
        json!({
            "type": "object",
            "properties": {
                "fen": { "type": "string", "description": "Position to look up, in FEN." },
                "limit": {
                    "type": "integer", "minimum": 1,
                    "description": "Max games to return (default 20)."
                }
            },
            "required": ["fen"]
        }),
        |app, user, args| async move { reference_games(app, user, args).await },
    )
}

async fn reference_games(app: AppState, user: CurrentUser, args: Value) -> ToolOutcome {
    let Some(fen) = fen_arg(&args) else {
        return ToolOutcome::error("Invalid arguments: missing string field `fen`.");
    };
    let limit = args
        .get("limit")
        .and_then(Value::as_u64)
        .unwrap_or(DEFAULT_REFERENCE_LIMIT);
    let service = PositionReportService::new(app.db.clone());
    match service.references(&user, &fen, Some(limit)).await {
        Ok(games) => json_outcome(&games),
        Err(e) => report_error(e),
    }
}

/// Extract a non-empty `fen` string argument.
fn fen_arg(args: &Value) -> Option<String> {
    match args.get("fen").and_then(Value::as_str) {
        Some(fen) if !fen.trim().is_empty() => Some(fen.to_string()),
        _ => None,
    }
}

/// Serialize a result to pretty JSON, or a non-leaking error outcome.
fn json_outcome<T: Serialize>(value: &T) -> ToolOutcome {
    match serde_json::to_string_pretty(value) {
        Ok(text) => ToolOutcome::ok(text),
        Err(_) => ToolOutcome::error("failed to serialise report"),
    }
}

/// Map a [`SearchError`] to a tool outcome without leaking DB internals.
fn report_error(error: SearchError) -> ToolOutcome {
    match error {
        SearchError::InvalidFen(msg) => ToolOutcome::error(format!("invalid FEN: {msg}")),
        SearchError::Serialize(_) | SearchError::Db(_) => {
            ToolOutcome::error("database query failed")
        }
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
    fn registers_the_db_tools() {
        let list = registry().list();
        let tools = list["tools"].as_array().expect("tools array");
        for expected in ["db_position_report", "db_reference_games"] {
            assert!(
                tools.iter().any(|t| t["name"] == expected),
                "missing tool {expected}"
            );
        }
    }

    #[test]
    fn missing_fen_is_rejected_before_any_query() {
        let outcome = fen_arg(&json!({})).map(|_| ()).is_none();
        assert!(outcome, "empty arguments must yield no FEN");
        assert!(fen_arg(&json!({ "fen": "  " })).is_none());
        assert_eq!(fen_arg(&json!({ "fen": "x" })).as_deref(), Some("x"));
    }

    #[test]
    fn db_errors_are_not_leaked() {
        let outcome = report_error(SearchError::InvalidFen("bad".to_string()));
        assert!(outcome.is_error);
        assert!(outcome.text.contains("invalid FEN"));
        let outcome = report_error(SearchError::Db(sea_orm::DbErr::Custom("secret".into())));
        assert_eq!(outcome.text, "database query failed");
    }
}
