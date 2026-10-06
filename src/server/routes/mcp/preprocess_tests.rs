//! Unit tests for the data tools in `preprocess.rs`.

use super::*;

fn registry() -> ToolRegistry {
    let mut registry = ToolRegistry::new();
    register(&mut registry);
    registry
}

#[test]
fn registers_the_preprocessing_tools() {
    let list = registry().list();
    let tools = list["tools"].as_array().expect("tools array");
    for expected in ["opening_tree", "danger_map", "position_concepts"] {
        assert!(
            tools.iter().any(|t| t["name"] == expected),
            "missing tool {expected}"
        );
    }
}

#[test]
fn danger_map_requires_a_spine_and_concepts_a_fen() {
    let list = registry().list();
    let tools = list["tools"].as_array().unwrap();
    let danger = tools
        .iter()
        .find(|t| t["name"] == "danger_map")
        .expect("danger_map tool");
    assert_eq!(danger["inputSchema"]["required"][0], "spine_pgn");
    assert_eq!(
        danger["inputSchema"]["properties"]["database_id"]["type"],
        "integer"
    );
    let concepts = tools
        .iter()
        .find(|t| t["name"] == "position_concepts")
        .expect("position_concepts tool");
    assert!(concepts["inputSchema"]["properties"]["moves"].is_object());
}

#[test]
fn opening_tree_has_no_required_args() {
    // It defaults the start position, so a no-arg call is valid.
    let list = registry().list();
    let tools = list["tools"].as_array().unwrap();
    let tree = tools
        .iter()
        .find(|t| t["name"] == "opening_tree")
        .expect("opening_tree tool");
    assert!(tree["inputSchema"].get("required").is_none());
}

#[test]
fn opening_tree_advertises_plan_and_threat_args() {
    let list = registry().list();
    let tools = list["tools"].as_array().unwrap();
    let props = tools
        .iter()
        .find(|t| t["name"] == "opening_tree")
        .map(|t| &t["inputSchema"]["properties"])
        .expect("opening_tree tool");
    assert_eq!(props["plan_lines"]["type"], "integer");
    assert_eq!(props["plan_lines"]["maximum"], MAX_PLAN_LINES);
    assert_eq!(props["threats"]["type"], "boolean");
}

#[test]
fn opening_tree_advertises_filter_args() {
    let list = registry().list();
    let tools = list["tools"].as_array().unwrap();
    let props = tools
        .iter()
        .find(|t| t["name"] == "opening_tree")
        .map(|t| &t["inputSchema"]["properties"])
        .expect("opening_tree tool");
    assert_eq!(props["player"]["type"], "string");
    assert_eq!(props["color"]["enum"], json!(["white", "black"]));
    assert_eq!(props["date_from"]["type"], "string");
    assert_eq!(props["date_to"]["type"], "string");
}

#[test]
fn missing_spine_pgn_is_rejected() {
    let outcome = position_concepts(json!({}));
    assert!(outcome.is_error);
    assert!(outcome.text.contains("`fen` or `moves`"));
}

#[test]
fn position_concepts_returns_a_concepts_block() {
    // The IQP-ish middlegame structure: pure, no engine/DB, so this exercises
    // the whole handler synchronously.
    let outcome = position_concepts(json!({
        "fen": "rnbqkbnr/pp3ppp/4p3/3p4/3P4/8/PPP2PPP/RNBQKBNR w KQkq - 0 1"
    }));
    assert!(!outcome.is_error, "got error: {}", outcome.text);
    let value: Value = serde_json::from_str(&outcome.text).expect("json");
    assert!(value.get("concepts").is_some());
}

#[test]
fn invalid_fen_is_reported_cleanly() {
    let outcome = position_concepts(json!({ "fen": "not-a-fen" }));
    assert!(outcome.is_error);
    assert!(outcome.text.contains("invalid FEN"));
}
