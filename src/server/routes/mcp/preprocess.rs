//! MCP study **preprocessing** tools (ADR-0027): the deterministic, engine + DB
//! grounded structural primitives that sit *behind* the study generators, now
//! exposed as plain data-returning MCP tools.
//!
//! They never call a language model. ADR-0027 draws the boundary at the MCP
//! transport: an MCP tool returns ground-truth data; the LLM that turns it into
//! prose lives on the **client** side of the boundary (an external agent, or the
//! embedded assistant — which drives this same registry). The model annotates and
//! persists the result through the `study_*` tools. So where the old
//! `generate_study` / `generate_danger_map` tools ran an LLM loop *inside* the
//! tool, these expose only the engine/DB stages the loop consumes:
//!
//! - `opening_tree` — the pruned, tagged [`VariationTree`] ([`build_variation_tree`]),
//! - `danger_map` — the engine-adjudicated [`DangerTree`] ([`walk_danger_spine_live`]),
//! - `position_concepts` — the pure pawn-structure / key-square [`Concepts`]
//!   ([`concepts_of_fen_with`]).
//!
//! Dispatch / JSON-RPC framing lives in [`super`].

use serde_json::{json, Value};

use super::db_tools::{fen_arg, json_outcome, opt_bounded_u64};
use super::study_tools::study_error;
use super::{Tool, ToolOutcome, ToolRegistry};
use crate::engine::{Limits, MAX_DEPTH, MAX_MOVETIME_MS};
use crate::pgn_tree::pgn::from_pgn_with_start;
use crate::position::{CastlingMode, STARTPOS_FEN};
use crate::search::position::PositionFilter;
use crate::search::report::PositionReportService;
use crate::server::identity::CurrentUser;
use crate::server::state::AppState;
use crate::studies::StudyService;
use crate::study_gen::features::concepts_of_fen_with;
use crate::study_gen::spine::{SpineConfig, SpineError};
use crate::study_gen::tree::{TreeConfig, TreeError};
use crate::study_gen::{
    apply_shapes, build_variation_tree, seed_study_from_danger, seed_study_from_tree,
    walk_danger_spine_live, EnginePlanAnalyzer, SeedOutcome, SeedParams, ShapeConfig,
    MAX_PLAN_LINES,
};

/// Studies are standard chess (mirrors [`crate::studies`]); every FEN here parses
/// castling rights the normal way.
const MODE: CastlingMode = CastlingMode::Standard;

/// Per-position engine search depth for `opening_tree` when unspecified; capped
/// server-side via [`Limits::clamped`].
const DEFAULT_TREE_DEPTH: u32 = 18;

/// Per-variation engine movetime budget (ms) for `danger_map` when unspecified;
/// capped server-side by the engine facade (ADR-0026).
const DEFAULT_DANGER_MOVETIME_MS: u64 = 500;

/// MultiPV line count for `danger_map` when unspecified; floored at 2 server-side
/// for the trap / only-move gap.
const DEFAULT_DANGER_MULTIPV: u16 = 2;

/// Upper bound on the `multipv` argument: more lines cost engine time for little
/// classifier gain, so reject absurd values rather than clamp silently.
const MAX_DANGER_MULTIPV: u64 = 16;

/// Register the study-preprocessing data tools into `registry`.
pub fn register(registry: &mut ToolRegistry) {
    registry.register(opening_tree_tool());
    registry.register(danger_map_tool());
    registry.register(position_concepts_tool());
}

// --- opening_tree --------------------------------------------------------

/// `opening_tree`: the pruned, engine + DB tagged variation tree for a position —
/// the deterministic opening skeleton the study generators annotate (issue #29).
fn opening_tree_tool() -> Tool {
    Tool::new(
        "opening_tree",
        "Build a pruned, engine- and database-tagged variation tree from a \
         position (FEN) — the deterministic opening skeleton, not an annotated \
         study. Walks the database-played continuations breadth-first, scores each \
         with the engine, and prunes by frequency + eval margin to a bounded, \
         teachable size. Every node carries the SAN, FEN, Zobrist key, engine \
         evaluation, database win/draw/loss + frequency stats, ECO name and \
         strategic concepts. Returns structured data only — no prose: annotate it \
         yourself and persist with the `study_*` tools. Optionally pins per-node \
         plan/threat board arrows (`shapes`: engine PV trajectories + hanging-piece \
         threats) you can carry straight into a study. Pass `save_as` to persist \
         the tree straight into a study server-side and get back just an id (no \
         tree JSON, no client-side PGN assembly) — then layer prose with \
         `study_annotate`. Scoped to your databases and the global ones. \
         Optionally narrow continuations to one player's games (`player`/`color`) \
         or a date range. Requires an engine configured.",
        json!({
            "type": "object",
            "properties": {
                "fen": { "type": "string", "description": "Start position in FEN; defaults to the standard opening." },
                "engine_depth": {
                    "type": "integer", "minimum": 1, "maximum": MAX_DEPTH,
                    "description": format!(
                        "Per-position engine search depth in plies (default {DEFAULT_TREE_DEPTH}); capped server-side."
                    )
                },
                "tree": {
                    "type": "object",
                    "description": "Optional tree pruning thresholds (max_depth, max_children, max_nodes, min_frequency, eval_margin_cp); partial overrides over the defaults. Set max_children_by_depth (e.g. [3,3,2,1]) to taper branching with depth — broad near the root, narrowing on deep main lines; the last entry repeats past its length and it overrides max_children."
                },
                "plan_lines": {
                    "type": "integer", "minimum": 0, "maximum": MAX_PLAN_LINES,
                    "description": format!(
                        "Pin the top-N engine PV lines as `plan1`..`plan{MAX_PLAN_LINES}` arrows on every node (0/omitted = off; capped at {MAX_PLAN_LINES})."
                    )
                },
                "threats": {
                    "type": "boolean",
                    "description": "Pin the static hanging-piece `threat` arrows on every node (default false)."
                },
                "player": {
                    "type": "string",
                    "description": "Restrict continuations to games featuring this player (substring match on name); either side unless `color` narrows it."
                },
                "color": {
                    "type": "string", "enum": ["white", "black"],
                    "description": "Restrict `player`'s games to one side; ignored without `player`."
                },
                "date_from": {
                    "type": "string",
                    "description": "Only games on/after this PGN date (inclusive)."
                },
                "date_to": {
                    "type": "string",
                    "description": "Only games on/before this PGN date (inclusive)."
                },
                "save_as": save_as_schema()
            }
        }),
        |app, user, args| async move { opening_tree(app, user, args).await },
    )
}

async fn opening_tree(app: AppState, user: CurrentUser, args: Value) -> ToolOutcome {
    let engine = match &app.engine_service {
        Some(engine) => engine.clone(),
        None => {
            return ToolOutcome::error(
                "No engine configured: start chess-base with --engine / CHESS_BASE_ENGINE.",
            )
        }
    };
    let start_fen = fen_arg(&args).unwrap_or_else(|| STARTPOS_FEN.to_string());

    let config: TreeConfig = match args.get("tree") {
        None | Some(Value::Null) => TreeConfig::default(),
        Some(value) => match serde_json::from_value(value.clone()) {
            Ok(config) => config,
            Err(e) => return ToolOutcome::error(format!("Invalid arguments: bad `tree`: {e}")),
        },
    };
    let depth = match opt_bounded_u64(&args, "engine_depth", MAX_DEPTH as u64) {
        Ok(depth) => depth.map(|d| d as u32).unwrap_or(DEFAULT_TREE_DEPTH),
        Err(msg) => return ToolOutcome::error(msg),
    };
    let save_as = match parse_save_as(&args) {
        Ok(save_as) => save_as,
        Err(msg) => return ToolOutcome::error(msg),
    };
    let limits = Limits::depth(depth).clamped();
    let plan_lines = match opt_bounded_u64(&args, "plan_lines", MAX_PLAN_LINES as u64) {
        Ok(lines) => lines.unwrap_or(0) as u8,
        Err(msg) => return ToolOutcome::error(msg),
    };
    let want_threats = args
        .get("threats")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let color = match args.get("color").and_then(Value::as_str) {
        None => None,
        Some("white") => Some(crate::search::position::Color::White),
        Some("black") => Some(crate::search::position::Color::Black),
        Some(other) => {
            return ToolOutcome::error(format!(
                "Invalid arguments: color must be 'white' or 'black', got '{other}'"
            ))
        }
    };
    let filter = PositionFilter {
        player: args
            .get("player")
            .and_then(Value::as_str)
            .map(str::to_string),
        color,
        date_from: args
            .get("date_from")
            .and_then(Value::as_str)
            .map(str::to_string),
        date_to: args
            .get("date_to")
            .and_then(Value::as_str)
            .map(str::to_string),
    };
    let reports = PositionReportService::new(app.db.clone());

    let mut tree = match build_variation_tree(
        &engine,
        &reports,
        &user,
        &start_fen,
        &config,
        &filter,
        limits.clone(),
        MODE,
    )
    .await
    {
        Ok(tree) => tree,
        Err(e) => return tree_error(e),
    };

    // Optionally pin engine "plan" + static "threat" arrows onto each node — pure
    // engine/DB data (ADR-0027), so the client can carry them into a study.
    let shapes = ShapeConfig {
        plan_lines,
        threats: want_threats,
    };
    if !shapes.is_off() {
        let analyzer =
            (plan_lines > 0).then(|| EnginePlanAnalyzer::new(&engine, limits, plan_lines as u16));
        let plans = analyzer
            .as_ref()
            .map(|a| a as &(dyn crate::study_gen::spine::MultiAnalyzer + Sync));
        apply_shapes(plans, &mut tree, &shapes, MODE).await;
    }

    // Persist straight into a study when asked (#158); otherwise return the tree
    // (now carrying any plan/threat shapes) as data.
    match save_as {
        None => json_outcome(&tree),
        Some(params) => {
            let studies = StudyService::new(app.db.clone());
            match seed_study_from_tree(&studies, &user, &tree, &params).await {
                Ok(outcome) => seed_outcome(&outcome),
                Err(e) => study_error(e),
            }
        }
    }
}

// --- danger_map ----------------------------------------------------------

/// `danger_map`: the engine-adjudicated danger tree for a repertoire spine — the
/// phase-2 walk (#139), unbundled from the LLM annotation the generator added.
fn danger_map_tool() -> Tool {
    Tool::new(
        "danger_map",
        "Walk an opening repertoire spine (PGN) for dangerous opponent replies and \
         return the engine-adjudicated danger tree — the deterministic classifier \
         output, not an annotated study. Every flagged node carries a Weapon / \
         Caution / Off-book role and the raw figures behind it (trap verdict, \
         only-move gap, human miss-rate, pawn-storm signal). Returns structured \
         data only — no prose: annotate it yourself and persist with the `study_*` \
         tools. Pass `save_as` to persist the danger tree straight into a study \
         server-side and get back just an id (no tree JSON) — then layer prose with \
         `study_annotate`. Scoped to your databases and the global ones. Requires \
         an engine configured.",
        json!({
            "type": "object",
            "properties": {
                "spine_pgn": { "type": "string", "description": "Repertoire spine as PGN movetext to walk for danger." },
                "fen": { "type": "string", "description": "Start position in FEN; defaults to the standard opening." },
                "movetime_ms": {
                    "type": "integer", "minimum": 1, "maximum": MAX_MOVETIME_MS,
                    "description": "Per-variation engine movetime budget in ms; capped server-side."
                },
                "multipv": {
                    "type": "integer", "minimum": 1, "maximum": MAX_DANGER_MULTIPV,
                    "description": "MultiPV line count; floored at 2 server-side for the trap / only-move gap."
                },
                "spine": {
                    "type": "object",
                    "description": "Optional walk shape + classifier thresholds (our_side, max_depth, min_frequency, max_replies, min_miss_rate, danger{…}, attack{…}); partial overrides over the defaults."
                },
                "save_as": save_as_schema()
            },
            "required": ["spine_pgn"]
        }),
        |app, user, args| async move { danger_map(app, user, args).await },
    )
}

async fn danger_map(app: AppState, user: CurrentUser, args: Value) -> ToolOutcome {
    let Some(spine_pgn) = args.get("spine_pgn").and_then(Value::as_str) else {
        return ToolOutcome::error("Invalid arguments: missing string field `spine_pgn`.");
    };
    let engine = match &app.engine_service {
        Some(engine) => engine.clone(),
        None => {
            return ToolOutcome::error(
                "No engine configured: start chess-base with --engine / CHESS_BASE_ENGINE.",
            )
        }
    };

    let start_fen = fen_arg(&args).unwrap_or_else(|| STARTPOS_FEN.to_string());
    let spine = match from_pgn_with_start(spine_pgn, &start_fen) {
        Ok(tree) => tree,
        Err(e) => return ToolOutcome::error(format!("Invalid arguments: bad `spine_pgn`: {e}")),
    };
    let config: SpineConfig = match args.get("spine") {
        None | Some(Value::Null) => SpineConfig::default(),
        Some(value) => match serde_json::from_value(value.clone()) {
            Ok(config) => config,
            Err(e) => return ToolOutcome::error(format!("Invalid arguments: bad `spine`: {e}")),
        },
    };
    let movetime_ms = match opt_bounded_u64(&args, "movetime_ms", MAX_MOVETIME_MS) {
        Ok(value) => value.unwrap_or(DEFAULT_DANGER_MOVETIME_MS),
        Err(msg) => return ToolOutcome::error(msg),
    };
    let multipv = match opt_bounded_u64(&args, "multipv", MAX_DANGER_MULTIPV) {
        Ok(value) => value.map(|n| n as u16).unwrap_or(DEFAULT_DANGER_MULTIPV),
        Err(msg) => return ToolOutcome::error(msg),
    };
    let save_as = match parse_save_as(&args) {
        Ok(save_as) => save_as,
        Err(msg) => return ToolOutcome::error(msg),
    };
    let reports = PositionReportService::new(app.db.clone());

    let danger = match walk_danger_spine_live(
        &engine,
        &reports,
        &user,
        &spine,
        &start_fen,
        &config,
        MODE,
        movetime_ms,
        multipv,
    )
    .await
    {
        Ok(danger) => danger,
        Err(e) => return spine_error(e),
    };

    match save_as {
        None => {
            // A flat digest of the tagged nodes (the tags are also embedded per
            // node in `tree`) — most dangerous lines first by walk order.
            let roles: Vec<Value> = danger
                .nodes
                .iter()
                .filter_map(|n| {
                    n.tag.as_ref().map(|tag| {
                        json!({
                            "node_id": n.id,
                            "san": n.san,
                            "kind": format!("{:?}", tag.kind),
                            "role": format!("{:?}", tag.role),
                        })
                    })
                })
                .collect();
            json_outcome(&json!({ "tree": danger, "roles": roles }))
        }
        Some(params) => {
            let studies = StudyService::new(app.db.clone());
            match seed_study_from_danger(&studies, &user, &danger, &params).await {
                Ok(outcome) => seed_outcome(&outcome),
                Err(e) => study_error(e),
            }
        }
    }
}

// --- position_concepts ---------------------------------------------------

/// `position_concepts`: the pure pawn-structure / key-square concept layer for a
/// position (issue #30) — no engine, no database, just the FEN.
fn position_concepts_tool() -> Tool {
    Tool::new(
        "position_concepts",
        "Classify the strategic concepts of a single position (FEN): named \
         pawn-structure types (IQP, hanging pawns, …), key squares with their \
         beneficiary, open and half-open files, and a flat tag summary. Pure \
         structural analysis — no engine or database needed. Distinct from \
         `analyse_position`'s material/phase feature tags: this is the \
         pawn-skeleton concept layer the study generators feed the annotator.",
        json!({
            "type": "object",
            "properties": {
                "fen": { "type": "string", "description": "Position to classify, in FEN." }
            },
            "required": ["fen"]
        }),
        |_app, _user, args| async move { position_concepts(args) },
    )
}

fn position_concepts(args: Value) -> ToolOutcome {
    let Some(fen) = fen_arg(&args) else {
        return ToolOutcome::error("Invalid arguments: missing string field `fen`.");
    };
    match concepts_of_fen_with(&fen, MODE) {
        Ok(concepts) => json_outcome(&json!({ "fen": fen, "concepts": concepts })),
        Err(e) => ToolOutcome::error(format!("invalid FEN: {e}")),
    }
}

// --- save_as (seed a study) ----------------------------------------------

/// JSON-schema fragment for the optional `save_as` argument shared by the data
/// tools: present ⇒ persist the built tree into a study and return its id; absent
/// ⇒ return the tree as data. Same ownership knobs as `study_create`.
fn save_as_schema() -> Value {
    json!({
        "type": "object",
        "description": "Persist the built tree straight into a study (no tree JSON in the response, no client-side PGN assembly). Omit to return the tree as data.",
        "properties": {
            "database_id": { "type": "integer", "description": "Database the new study belongs to." },
            "name": { "type": "string", "description": "Name for the new study." },
            "global": { "type": "boolean", "description": "Make it a global (admin-managed) study; requires admin." }
        },
        "required": ["database_id", "name"]
    })
}

/// Parse the optional `save_as` argument into [`SeedParams`]. Absent / null ⇒
/// `None` (return the tree as today). Present but malformed ⇒ a client error,
/// surfaced before the engine runs so a bad request is cheap to reject.
fn parse_save_as(args: &Value) -> Result<Option<SeedParams>, String> {
    let save_as = match args.get("save_as") {
        None | Some(Value::Null) => return Ok(None),
        Some(value) => value,
    };
    let Some(database_id) = save_as.get("database_id").and_then(Value::as_i64) else {
        return Err("Invalid arguments: `save_as` needs an integer `database_id`.".into());
    };
    let Some(name) = save_as.get("name").and_then(Value::as_str) else {
        return Err("Invalid arguments: `save_as` needs a string `name`.".into());
    };
    let global = save_as
        .get("global")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    Ok(Some(SeedParams {
        database_id: database_id as i32,
        name: name.to_string(),
        global,
    }))
}

/// The seed response: just the new study id and its node count — never the tree
/// itself (that round-trip is the whole problem this avoids, issue #155).
fn seed_outcome(outcome: &SeedOutcome) -> ToolOutcome {
    json_outcome(&json!({
        "study_id": outcome.study.id,
        "node_count": outcome.node_count,
    }))
}

// --- error mapping -------------------------------------------------------

/// Map a [`TreeError`] onto a tool outcome without leaking engine/DB internals.
fn tree_error(error: TreeError) -> ToolOutcome {
    match error {
        TreeError::InvalidFen(msg) => ToolOutcome::error(format!("invalid FEN: {msg}")),
        TreeError::Source(_) => ToolOutcome::error("could not read the position from the database"),
    }
}

/// Map a [`SpineError`] onto a tool outcome without leaking engine/DB internals.
fn spine_error(error: SpineError) -> ToolOutcome {
    match error {
        SpineError::InvalidFen(msg) => ToolOutcome::error(format!("invalid FEN: {msg}")),
        SpineError::Source(_) => {
            ToolOutcome::error("could not read the position from the database")
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
        let concepts = tools
            .iter()
            .find(|t| t["name"] == "position_concepts")
            .expect("position_concepts tool");
        assert_eq!(concepts["inputSchema"]["required"][0], "fen");
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
        assert!(outcome.text.contains("missing string field `fen`"));
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
}
