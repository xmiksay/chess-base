//! MCP database tools (issue #28): the *pre-chewed* query surface. Each tool
//! returns synthesized data — ECO, per-move win/draw/loss + frequency/score,
//! transpositions, reference games — from [`PositionReportService`], scoped to
//! the authenticated caller (ADR-0016). The model consumes conclusions; it never
//! computes them (ADR-0009). Dispatch/JSON-RPC framing lives in [`super`].

use serde::Serialize;
use serde_json::{json, Value};

use super::{Tool, ToolOutcome, ToolRegistry};
use crate::databases::{DatabaseError, DatabaseService};
use crate::engine::{Limits, MAX_DEPTH, MAX_MOVETIME_MS};
use crate::games::{GameError, GameService, MAX_LIMIT as MAX_GAME_LIMIT};
use crate::search::report::PositionReportService;
use crate::search::SearchError;
use crate::server::identity::CurrentUser;
use crate::server::state::AppState;

/// Reference games returned when the caller gives no explicit `limit`.
const DEFAULT_REFERENCE_LIMIT: u64 = 20;

/// Cap on `db_reference_games`' `limit`: a huge value would scan/serialise an
/// unbounded result set (issue #93).
const MAX_REFERENCE_LIMIT: u64 = 200;

/// Register the pre-chewed DB query tools into `registry`.
pub fn register(registry: &mut ToolRegistry) {
    registry.register(position_report_tool());
    registry.register(reference_games_tool());
    registry.register(list_databases_tool());
    registry.register(list_games_tool());
    registry.register(read_game_tool());
}

/// `list_databases`: the caller's databases plus the global ones, with game
/// counts — so an agent can discover a `database_id` before building a study.
fn list_databases_tool() -> Tool {
    Tool::new(
        "list_databases",
        "List the databases (game collections) you can see — your own plus the \
         global ones — each with its id, name, owner, `global` flag and game \
         count. Use this to discover the `database_id` that `study_create`, \
         `study_import_pgn`, `generate_study` and `db_list_games` need.",
        json!({ "type": "object", "properties": {} }),
        |app, user, _args| async move { list_databases(app, user).await },
    )
}

async fn list_databases(app: AppState, user: CurrentUser) -> ToolOutcome {
    let service = DatabaseService::new(app.db.clone());
    match service.list_with_counts(&user).await {
        Ok(rows) => {
            let views: Vec<Value> = rows
                .into_iter()
                .map(|(d, game_count)| {
                    json!({
                        "id": d.id,
                        "name": d.name,
                        "owner_id": d.owner_id,
                        "global": d.owner_id.is_none(),
                        "game_count": game_count,
                    })
                })
                .collect();
            json_outcome(&views)
        }
        Err(e) => database_error(e),
    }
}

/// `db_list_games`: one keyset page of the games in a database.
fn list_games_tool() -> Tool {
    Tool::new(
        "db_list_games",
        "List the games in a database (oldest-first, keyset-paginated). Returns \
         game headers (players, date, result, ECO, Elo, ply count) plus a \
         `next_cursor` to pass back as `after` for the next page. Scoped to your \
         databases and the global ones. Discover the `database_id` via \
         `list_databases`.",
        json!({
            "type": "object",
            "properties": {
                "database_id": { "type": "integer", "description": "Database to list games from." },
                "after": {
                    "type": "integer", "minimum": 1,
                    "description": "Keyset cursor: the last game id of the previous page (optional)."
                },
                "limit": {
                    "type": "integer", "minimum": 1, "maximum": MAX_GAME_LIMIT,
                    "description": "Max games to return (default 50); capped server-side."
                }
            },
            "required": ["database_id"]
        }),
        |app, user, args| async move { list_games(app, user, args).await },
    )
}

async fn list_games(app: AppState, user: CurrentUser, args: Value) -> ToolOutcome {
    let Some(database_id) = args.get("database_id").and_then(Value::as_i64) else {
        return ToolOutcome::error("Invalid arguments: missing integer field `database_id`.");
    };
    let after = match opt_bounded_u64(&args, "after", i32::MAX as u64) {
        Ok(value) => value.map(|n| n as i32),
        Err(msg) => return ToolOutcome::error(msg),
    };
    let limit = match opt_bounded_u64(&args, "limit", MAX_GAME_LIMIT) {
        Ok(limit) => limit,
        Err(msg) => return ToolOutcome::error(msg),
    };
    let service = GameService::new(app.db.clone());
    match service.list(&user, database_id as i32, after, limit).await {
        Ok(page) => json_outcome(&page),
        Err(e) => game_error(e),
    }
}

/// `db_read_game`: a single game by id, with its PGN movetext.
fn read_game_tool() -> Tool {
    Tool::new(
        "db_read_game",
        "Fetch one game by id, with its full PGN movetext and header roster \
         (players, result, ECO, variant, start FEN). Scoped to your databases and \
         the global ones. Discover game ids via `db_list_games`.",
        json!({
            "type": "object",
            "properties": {
                "game_id": { "type": "integer", "description": "Game to fetch." }
            },
            "required": ["game_id"]
        }),
        |app, user, args| async move { read_game(app, user, args).await },
    )
}

async fn read_game(app: AppState, user: CurrentUser, args: Value) -> ToolOutcome {
    let Some(game_id) = args.get("game_id").and_then(Value::as_i64) else {
        return ToolOutcome::error("Invalid arguments: missing integer field `game_id`.");
    };
    let service = GameService::new(app.db.clone());
    match service.get(&user, game_id as i32).await {
        Ok(game) => json_outcome(&game),
        Err(e) => game_error(e),
    }
}

/// Map a [`DatabaseError`] to a tool outcome without leaking DB internals.
fn database_error(error: DatabaseError) -> ToolOutcome {
    match error {
        DatabaseError::Db(_) => ToolOutcome::error("database query failed"),
        other => ToolOutcome::error(other.to_string()),
    }
}

/// Map a [`GameError`] to a tool outcome without leaking DB internals.
fn game_error(error: GameError) -> ToolOutcome {
    match error {
        GameError::NotFound => ToolOutcome::error("game not found"),
        GameError::Db(_) => ToolOutcome::error("database query failed"),
    }
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
                    "type": "integer", "minimum": 1, "maximum": MAX_REFERENCE_LIMIT,
                    "description": "Max games to return (default 20); capped server-side."
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
    let limit = match opt_bounded_u64(&args, "limit", MAX_REFERENCE_LIMIT) {
        Ok(limit) => limit.unwrap_or(DEFAULT_REFERENCE_LIMIT),
        Err(msg) => return ToolOutcome::error(msg),
    };
    let service = PositionReportService::new(app.db.clone());
    match service.references(&user, &fen, Some(limit)).await {
        Ok(games) => json_outcome(&games),
        Err(e) => report_error(e),
    }
}

/// Extract a non-empty `fen` string argument. Shared by the analysis/engine
/// tools, which turn a `None` into their own "missing `fen`" tool error.
pub(super) fn fen_arg(args: &Value) -> Option<String> {
    match args.get("fen").and_then(Value::as_str) {
        Some(fen) if !fen.trim().is_empty() => Some(fen.to_string()),
        _ => None,
    }
}

/// Extract the engine search [`Limits`] (`depth` / `movetime_ms`) from a tool's
/// arguments. Shared by the engine and interactive-analysis tools. Values `< 1`
/// are rejected and oversized ones clamped (issue #93) — clamping the `depth`
/// `u64` before the `u32` cast also avoids a silent wrap. The service applies the
/// same clamp again as defence in depth.
pub(super) fn limits_arg(args: &Value) -> Result<Limits, String> {
    let depth = opt_bounded_u64(args, "depth", MAX_DEPTH as u64)?.map(|d| d as u32);
    let movetime_ms = opt_bounded_u64(args, "movetime_ms", MAX_MOVETIME_MS)?;
    Ok(Limits {
        depth,
        movetime_ms,
        nodes: None,
    })
}

/// Parse an optional positive-integer argument: absent ⇒ `Ok(None)`; a present
/// value `< 1` (or non-integer) ⇒ `Err`; otherwise clamped to `max`.
pub(super) fn opt_bounded_u64(args: &Value, key: &str, max: u64) -> Result<Option<u64>, String> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => {
            let n = value
                .as_u64()
                .filter(|&n| n >= 1)
                .ok_or_else(|| format!("Invalid arguments: `{key}` must be an integer >= 1."))?;
            Ok(Some(n.min(max)))
        }
    }
}

/// Serialize a result to pretty JSON, or a non-leaking error outcome.
pub(super) fn json_outcome<T: Serialize>(value: &T) -> ToolOutcome {
    match serde_json::to_string_pretty(value) {
        Ok(text) => ToolOutcome::ok(text),
        Err(_) => ToolOutcome::error("failed to serialise report"),
    }
}

/// Map a [`SearchError`] to a tool outcome without leaking DB internals.
pub(super) fn report_error(error: SearchError) -> ToolOutcome {
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
    fn limits_are_clamped_and_never_wrap() {
        // Way past u32 and the movetime cap: clamped, not wrapped.
        let limits = limits_arg(&json!({ "depth": 100_000, "movetime_ms": 600_000 })).unwrap();
        assert_eq!(limits.depth, Some(MAX_DEPTH));
        assert_eq!(limits.movetime_ms, Some(MAX_MOVETIME_MS));

        // In-range values pass through untouched.
        let limits = limits_arg(&json!({ "depth": 18, "movetime_ms": 5_000 })).unwrap();
        assert_eq!(limits.depth, Some(18));
        assert_eq!(limits.movetime_ms, Some(5_000));

        // Omitted ⇒ unset (the service supplies a default depth).
        let limits = limits_arg(&json!({})).unwrap();
        assert_eq!(limits.depth, None);
        assert_eq!(limits.movetime_ms, None);
    }

    #[test]
    fn limits_below_one_are_rejected() {
        assert!(limits_arg(&json!({ "depth": 0 })).is_err());
        assert!(limits_arg(&json!({ "movetime_ms": 0 })).is_err());
        // Negative numbers are not valid u64s either.
        assert!(limits_arg(&json!({ "depth": -5 })).is_err());
    }

    #[test]
    fn reference_limit_is_clamped_and_validated() {
        assert_eq!(
            opt_bounded_u64(&json!({ "limit": 10_000 }), "limit", MAX_REFERENCE_LIMIT),
            Ok(Some(MAX_REFERENCE_LIMIT))
        );
        assert_eq!(
            opt_bounded_u64(&json!({}), "limit", MAX_REFERENCE_LIMIT),
            Ok(None)
        );
        assert!(opt_bounded_u64(&json!({ "limit": 0 }), "limit", MAX_REFERENCE_LIMIT).is_err());
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
