//! The position argument shared by every FEN-taking tool (ADR-0056): a `fen`,
//! a `moves` SAN list, or both (the moves are played from the FEN, else from
//! the standard start). Models write FEN badly — a hand-built string silently
//! names an unreachable position and every stats tool answers "0 games" — so
//! `moves` lets the server do the replay and fail loudly on the exact bad move.

use serde::Serialize;
use serde_json::{json, Value};

use crate::position::{replay, STARTPOS_FEN};
use shakmaty::CastlingMode;

/// Schema properties for `fen` + `moves`, merged into each tool's `properties`.
/// Neither is `required` in the schema (it can't express "one of" portably
/// across providers); [`require_position`] enforces it.
pub(super) fn position_props(what: &str) -> Value {
    json!({
        "fen": {
            "type": "string",
            "description": format!(
                "{what}, in FEN. Prefer `moves` unless you have an exact FEN from a tool."
            )
        },
        "moves": moves_prop()
    })
}

/// The `moves` property alone, for tools whose schema is otherwise hand-built.
pub(super) fn moves_prop() -> Value {
    json!({
        "type": "array",
        "items": { "type": "string" },
        "description": "Moves in SAN from the start position (or from `fen` when both \
                        are given), e.g. [\"d4\",\"Nf6\",\"c4\",\"e6\"]. The server \
                        replays them, so the position is always legal."
    })
}

/// Build an object schema from [`position_props`] plus the tool's own extras.
pub(super) fn position_schema(what: &str, extra: Value) -> Value {
    let mut props = position_props(what);
    if let (Some(props), Value::Object(extra)) = (props.as_object_mut(), extra) {
        props.extend(extra);
    }
    json!({ "type": "object", "properties": props })
}

/// Resolve `fen`/`moves` to a FEN; `Ok(None)` when neither is given. An
/// illegal move or a malformed `moves` is the caller's error, naming the move.
pub(super) fn position_arg(args: &Value) -> Result<Option<String>, String> {
    let fen = match args.get("fen").and_then(Value::as_str) {
        Some(f) if !f.trim().is_empty() => Some(f.trim().to_string()),
        _ => None,
    };
    let moves = match args.get("moves") {
        None | Some(Value::Null) => return Ok(fen),
        Some(Value::Array(items)) => items
            .iter()
            .map(|m| m.as_str().map(str::trim).filter(|s| !s.is_empty()))
            .collect::<Option<Vec<_>>>()
            .ok_or("Invalid arguments: `moves` must be an array of SAN strings.")?,
        Some(_) => {
            return Err("Invalid arguments: `moves` must be an array of SAN strings.".into())
        }
    };
    let start = fen.as_deref().unwrap_or(STARTPOS_FEN);
    // Replay one move at a time only to name the failing index; `replay` stops
    // at the first bad move but doesn't say where.
    match replay(start, &moves, CastlingMode::Standard) {
        Ok(plies) => Ok(Some(
            plies
                .last()
                .map_or_else(|| start.to_string(), |p| p.fen.clone()),
        )),
        Err(e) => {
            let at = (0..moves.len())
                .find(|&i| replay(start, &moves[..=i], CastlingMode::Standard).is_err())
                .unwrap_or(0);
            let san = moves.get(at).copied().unwrap_or_default();
            Err(format!(
                "Invalid arguments: move {} (`{san}`) of `moves` cannot be played: {e}",
                at + 1
            ))
        }
    }
}

/// [`position_arg`] for tools that need a position.
pub(super) fn require_position(args: &Value) -> Result<String, String> {
    position_arg(args)?
        .ok_or_else(|| "Invalid arguments: give `fen` or `moves` for the position.".to_string())
}

/// [`position_arg`] for tools rooted at the start position by default.
pub(super) fn position_or_start(args: &Value) -> Result<String, String> {
    Ok(position_arg(args)?.unwrap_or_else(|| STARTPOS_FEN.to_string()))
}

/// Serialize `value` as an object with the resolved `fen` added, so the model
/// sees (and can reuse) the exact position a `moves` call resolved to.
pub(super) fn with_fen(value: &impl Serialize, fen: &str) -> Value {
    match serde_json::to_value(value) {
        Ok(Value::Object(mut map)) => {
            map.entry("fen").or_insert_with(|| json!(fen));
            Value::Object(map)
        }
        Ok(other) => json!({ "fen": fen, "result": other }),
        Err(e) => json!({ "fen": fen, "error": e.to_string() }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const AFTER_D4_NF6: &str = "rnbqkb1r/pppppppp/5n2/8/3P4/8/PPP1PPPP/RNBQKBNR w KQkq - 1 2";

    #[test]
    fn moves_replay_from_the_start_position() {
        let fen = position_arg(&json!({ "moves": ["d4", "Nf6"] })).unwrap();
        assert_eq!(fen.as_deref(), Some(AFTER_D4_NF6));
    }

    #[test]
    fn moves_replay_from_a_given_fen() {
        let after_d4 = position_arg(&json!({ "moves": ["d4"] })).unwrap().unwrap();
        let fen = position_arg(&json!({ "fen": after_d4, "moves": ["Nf6"] })).unwrap();
        assert_eq!(fen.as_deref(), Some(AFTER_D4_NF6));
    }

    #[test]
    fn fen_alone_passes_through_and_nothing_is_none() {
        assert_eq!(
            position_arg(&json!({ "fen": " x " })).unwrap().as_deref(),
            Some("x")
        );
        assert_eq!(position_arg(&json!({})).unwrap(), None);
        assert_eq!(position_arg(&json!({ "fen": "  " })).unwrap(), None);
        assert_eq!(
            position_arg(&json!({ "moves": [] })).unwrap().as_deref(),
            Some(STARTPOS_FEN)
        );
    }

    #[test]
    fn an_illegal_move_is_named_by_position() {
        let err = position_arg(&json!({ "moves": ["d4", "Nf6", "Ke3"] })).unwrap_err();
        assert!(err.contains("move 3 (`Ke3`)"), "{err}");
    }

    #[test]
    fn malformed_moves_are_rejected() {
        assert!(position_arg(&json!({ "moves": "d4 Nf6" })).is_err());
        assert!(position_arg(&json!({ "moves": ["d4", 5] })).is_err());
        assert!(position_arg(&json!({ "moves": ["d4", ""] })).is_err());
    }

    #[test]
    fn require_and_default_helpers() {
        assert!(require_position(&json!({}))
            .unwrap_err()
            .contains("`fen` or `moves`"));
        assert_eq!(position_or_start(&json!({})).unwrap(), STARTPOS_FEN);
    }

    #[test]
    fn with_fen_adds_to_objects_and_wraps_others() {
        assert_eq!(
            with_fen(&json!({ "a": 1 }), "F"),
            json!({ "a": 1, "fen": "F" })
        );
        assert_eq!(
            with_fen(&json!({ "fen": "kept" }), "F"),
            json!({ "fen": "kept" })
        );
        assert_eq!(
            with_fen(&json!([1]), "F"),
            json!({ "fen": "F", "result": [1] })
        );
    }

    #[test]
    fn schema_merges_extras() {
        let s = position_schema("Position", json!({ "limit": { "type": "integer" } }));
        let props = s["properties"].as_object().unwrap();
        assert!(props.contains_key("fen") && props.contains_key("moves"));
        assert!(props.contains_key("limit"));
        assert!(s.get("required").is_none());
    }
}
