//! MCP header search + threats tools (issue #183): metadata/header search over
//! games, and the hanging-piece threat scan. Thin wrappers over
//! [`HeaderSearchService`] / [`threats_standard`], mirroring `search/routes.rs`'s
//! `/api/search/headers` and `threats/routes.rs`'s `/api/threats`.

use serde_json::{json, Value};

use super::db_tools::json_outcome;
use super::{Tool, ToolOutcome, ToolRegistry};
use crate::search::headers::{HeaderParams, HeaderQuery, HeaderSearchError, HeaderSearchService};
use crate::server::identity::CurrentUser;
use crate::server::state::AppState;
use crate::threats::threats_standard;

/// Register the search + threats tools into `registry`.
pub fn register(registry: &mut ToolRegistry) {
    registry.register(search_headers_tool());
    registry.register(position_threats_tool());
}

/// `search_headers`: keyset-paginated metadata search over games.
fn search_headers_tool() -> Tool {
    Tool::new(
        "search_headers",
        "Search games by header metadata: player (either side, or `color`- \
         restricted), event, ECO prefix, date range, result, a single database \
         (`database_id`) and ELO range (`elo_min`/`elo_max` — both players must \
         be rated inside the bounds) — keyset-paginated on a stable cursor. \
         `sort: elo` orders by the players' average rating, unrated games last. \
         Pass the `next_cursor` from a page back as `cursor` to fetch the next \
         one (absent once exhausted). Scoped to your databases and the global \
         ones.",
        json!({
            "type": "object",
            "properties": {
                "player": { "type": "string", "description": "Player name (substring match, either side unless `color` narrows it)." },
                "color": { "type": "string", "enum": ["white", "black"], "description": "Restrict `player` to one side." },
                "event": { "type": "string", "description": "Event name (substring match)." },
                "eco": { "type": "string", "description": "ECO code prefix, e.g. `B90`." },
                "date_from": { "type": "string", "description": "Only games on/after this PGN date (inclusive)." },
                "date_to": { "type": "string", "description": "Only games on/before this PGN date (inclusive)." },
                "result": { "type": "string", "description": "Exact result, e.g. `1-0`." },
                "database_id": { "type": "integer", "description": "Restrict to one database id. Must be yours or a global one; anything else is not-found." },
                "elo_min": { "type": "integer", "description": "Minimum ELO (inclusive): BOTH players must be rated at or above it. Games missing either rating are excluded whenever a bound is set." },
                "elo_max": { "type": "integer", "description": "Maximum ELO (inclusive): BOTH players must be rated at or below it. Games missing either rating are excluded whenever a bound is set." },
                "sort": { "type": "string", "enum": ["date", "id", "elo"], "description": "Sort field (default date). `elo` orders by the players' average rating; games missing either rating sort last." },
                "dir": { "type": "string", "enum": ["asc", "desc"], "description": "Sort direction (default desc)." },
                "limit": { "type": "integer", "minimum": 1, "description": "Max games per page (default 50; capped server-side)." },
                "cursor": { "type": "string", "description": "Opaque cursor from a previous page's `next_cursor`." }
            }
        }),
        |app, user, args| async move { search_headers(app, user, args).await },
    )
}

async fn search_headers(app: AppState, user: CurrentUser, args: Value) -> ToolOutcome {
    let params = HeaderParams {
        player: str_arg(&args, "player"),
        color: str_arg(&args, "color"),
        event: str_arg(&args, "event"),
        eco: str_arg(&args, "eco"),
        date_from: str_arg(&args, "date_from"),
        date_to: str_arg(&args, "date_to"),
        result: str_arg(&args, "result"),
        database_id: int_arg(&args, "database_id"),
        elo_min: int_arg(&args, "elo_min"),
        elo_max: int_arg(&args, "elo_max"),
        sort: str_arg(&args, "sort"),
        dir: str_arg(&args, "dir"),
        limit: args.get("limit").and_then(Value::as_u64),
        cursor: str_arg(&args, "cursor"),
    };
    let query = match HeaderQuery::try_from(params) {
        Ok(query) => query,
        Err(e) => return header_error(e),
    };
    let service = HeaderSearchService::new(app.db.clone());
    match service.search(&user, &query).await {
        Ok(page) => json_outcome(&page),
        Err(e) => header_error(e),
    }
}

/// Extract a non-empty string argument.
fn str_arg(args: &Value, key: &str) -> Option<String> {
    args.get(key).and_then(Value::as_str).map(str::to_string)
}

/// Extract an i32 argument; a value outside i32 can't match anything anyway.
fn int_arg(args: &Value, key: &str) -> Option<i32> {
    args.get(key)
        .and_then(Value::as_i64)
        .and_then(|v| i32::try_from(v).ok())
}

/// Map a [`HeaderSearchError`] to a tool outcome without leaking DB internals.
fn header_error(error: HeaderSearchError) -> ToolOutcome {
    match error {
        HeaderSearchError::BadRequest(msg) => {
            ToolOutcome::error(format!("invalid arguments: {msg}"))
        }
        HeaderSearchError::InvalidCursor => ToolOutcome::error("invalid arguments: bad `cursor`"),
        HeaderSearchError::NotFound => ToolOutcome::error("database not found"),
        HeaderSearchError::Serialize(_) | HeaderSearchError::Db(_) => {
            ToolOutcome::error("database query failed")
        }
    }
}

/// `position_threats`: hanging-piece red arrows for the side to move.
fn position_threats_tool() -> Tool {
    Tool::new(
        "position_threats",
        "Scan a position (FEN) for the side-to-move's hanging pieces: pieces \
         attacked and either undefended or defended only behind a cheaper \
         attacker. Returns board shapes (red arrows from attacker to target). A \
         cheap static scan — no engine search, so it ignores pins/X-rays by \
         design.",
        json!({
            "type": "object",
            "properties": {
                "fen": { "type": "string", "description": "Position to scan, in FEN." }
            },
            "required": ["fen"]
        }),
        |_app, _user, args| async move { position_threats(args) },
    )
}

fn position_threats(args: Value) -> ToolOutcome {
    let Some(fen) = super::db_tools::fen_arg(&args) else {
        return ToolOutcome::error("Invalid arguments: missing string field `fen`.");
    };
    match threats_standard(&fen) {
        Ok(shapes) => json_outcome(&shapes),
        Err(e) => ToolOutcome::error(format!("invalid FEN: {e}")),
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
    fn registers_the_search_tools() {
        let list = registry().list();
        let tools = list["tools"].as_array().expect("tools array");
        for expected in ["search_headers", "position_threats"] {
            assert!(
                tools.iter().any(|t| t["name"] == expected),
                "missing tool {expected}"
            );
        }
    }

    #[test]
    fn missing_fen_is_rejected() {
        let outcome = position_threats(json!({}));
        assert!(outcome.is_error);
        assert!(outcome.text.contains("missing string field `fen`"));
    }

    #[test]
    fn threats_are_returned_for_a_valid_fen() {
        // White queen on d1 hangs to the black bishop on a4 down the a4-d1 diagonal.
        let outcome = position_threats(json!({
            "fen": "4k3/8/8/8/b7/8/8/3QK3 w - - 0 1"
        }));
        assert!(!outcome.is_error, "got error: {}", outcome.text);
        let shapes: Value = serde_json::from_str(&outcome.text).expect("json");
        assert!(shapes.as_array().is_some());
    }
}
