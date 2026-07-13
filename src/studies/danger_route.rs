//! HTTP surface for the danger-map walk (issue #141, ADR-0026 phase 4):
//!
//! - `POST /api/studies/generate-danger-map` — a thin caller over
//!   [`generate_danger_study_live`] mirroring `POST /api/studies/generate`: it
//!   parses the repertoire spine PGN into a [`MoveTree`], runs the phase-2/3 LLM
//!   orchestrator scoped to the caller, and persists an annotated study.
//! - `POST /api/studies/danger-map` (issue #156) — the **engine-only** sibling: a
//!   thin caller over [`walk_danger_spine_live`] (symmetric to the MCP `danger_map`
//!   tool, ADR-0027) that returns the raw [`DangerTree`] for the spine without ever
//!   touching a language model, so the danger overlay works on a no-key install.
//!
//! All engine/DB/LLM internals stay behind the transport-agnostic error mapping.

use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::post,
    Json, Router,
};
use serde::{Deserialize, Serialize};

use crate::pgn_tree::pgn::from_pgn_with_start;
use crate::pgn_tree::Eval;
use crate::position::{CastlingMode, STARTPOS_FEN};
use crate::search::report::PositionReportService;
use crate::server::error::error_response;
use crate::server::identity::CurrentUser;
use crate::server::state::AppState;
use crate::studies::StudyService;
use crate::study_gen::spine::{SpineConfig, SpineError};
use crate::study_gen::{
    generate_danger_study_live, walk_danger_spine_live, DangerStudyOutcome, DangerStudyParams,
    DangerTree,
};

/// Studies are standard chess (mirrors [`crate::studies`] and the MCP tool); every
/// FEN here parses castling rights the normal way.
const MODE: CastlingMode = CastlingMode::Standard;

/// Per-variation engine movetime budget (ms) when the request omits it; capped
/// server-side by the engine facade (ADR-0026).
const DEFAULT_MOVETIME_MS: u64 = 500;

/// MultiPV line count when the request omits it. Floored at 2 inside
/// [`crate::study_gen::EngineMultiAnalyzer`] for the trap / only-move gap.
const DEFAULT_MULTIPV: u16 = 2;

/// Danger-map routes, merged into the main API router.
pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/api/studies/generate-danger-map", post(generate))
        .route("/api/studies/danger-map", post(danger_map))
        .with_state(state)
}

/// Body for `POST /api/studies/generate-danger-map`. Only the target database,
/// name and repertoire spine are required; the walk shape and classifier
/// thresholds default to [`SpineConfig::default`] and accept partial overrides.
#[derive(Deserialize)]
struct DangerMapBody {
    database_id: i32,
    name: String,
    /// The repertoire spine as PGN movetext to walk for danger.
    spine_pgn: String,
    /// Create a global (admin-owned) study; requires admin. Defaults to false.
    #[serde(default)]
    global: bool,
    /// FEN the walk starts from; defaults to the standard start position.
    #[serde(default)]
    start_fen: Option<String>,
    /// LLM model id; defaults to the provider's default model.
    #[serde(default)]
    model: Option<String>,
    /// Walk shape + classifier thresholds; partial overrides over the defaults.
    #[serde(default)]
    spine: SpineConfig,
    /// Per-variation engine movetime budget (ms); capped server-side.
    #[serde(default)]
    movetime_ms: Option<u64>,
    /// MultiPV line count (floored at 2 server-side).
    #[serde(default)]
    multipv: Option<u16>,
}

/// Summary returned by `generate`: the created study, what the verification loop
/// dropped, and the engine-adjudicated danger roles, so the caller can fetch the
/// full tree via `GET /api/studies/{id}`.
#[derive(Serialize)]
struct DangerMapView {
    id: i32,
    database_id: i32,
    name: String,
    global: bool,
    /// Nodes in the committed annotated tree.
    node_count: usize,
    /// How many model claims/glyphs ground truth rejected (never committed).
    rejected: usize,
    /// Danger role tags carried into the study, most dangerous lines first.
    roles: Vec<RoleView>,
}

/// One engine-adjudicated danger role surfaced on the result.
#[derive(Serialize)]
struct RoleView {
    node_id: usize,
    san: Option<String>,
    kind: String,
    role: String,
    /// White-perspective eval of the position this node reaches (issue #177);
    /// absent for an Off-book node (no search ran on its own position).
    #[serde(skip_serializing_if = "Option::is_none")]
    eval: Option<Eval>,
}

impl From<&DangerStudyOutcome> for DangerMapView {
    fn from(outcome: &DangerStudyOutcome) -> Self {
        Self {
            id: outcome.study.id,
            database_id: outcome.study.database_id,
            name: outcome.study.name.clone(),
            global: outcome.study.owner_id.is_none(),
            node_count: outcome.node_count,
            rejected: outcome.rejected.len(),
            roles: outcome
                .roles
                .iter()
                .map(|r| RoleView {
                    node_id: r.node_id,
                    san: r.san.clone(),
                    kind: format!("{:?}", r.kind),
                    role: format!("{:?}", r.role),
                    eval: r.eval,
                })
                .collect(),
        }
    }
}

/// Generate an annotated danger study from a repertoire spine
/// (`POST /api/studies/generate-danger-map`). Thin caller over
/// [`generate_danger_study_live`]: spine walk → fold to a tree → batch LLM
/// annotation + verification → persist, all scoped to the caller. Requires both
/// an engine and an LLM provider; failures surface a clean status without leaking
/// engine/DB/LLM internals.
async fn generate(
    State(state): State<AppState>,
    user: CurrentUser,
    Json(body): Json<DangerMapBody>,
) -> Result<Response, Response> {
    // Validate client input (the spine PGN) before probing operator config, so a
    // malformed request is a clean 400 regardless of whether an engine is wired.
    let start_fen = body
        .start_fen
        .filter(|fen| !fen.trim().is_empty())
        .unwrap_or_else(|| STARTPOS_FEN.to_string());
    let spine = from_pgn_with_start(&body.spine_pgn, &start_fen)
        .map_err(|e| error_response(StatusCode::BAD_REQUEST, format!("invalid spine PGN: {e}")))?;

    // A missing engine / model is an operator-configuration gap, not a leaked
    // internal — surface the guidance verbatim (like `POST /generate`).
    let engine = state.engine_service.as_ref().ok_or_else(|| {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            "No engine configured: start chess-base with --engine / CHESS_BASE_ENGINE.",
        )
            .into_response()
    })?;
    let provider = state.llm_provider.as_ref().ok_or_else(|| {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            "No language model configured: set ANTHROPIC_API_KEY to enable study generation.",
        )
            .into_response()
    })?;

    let params = DangerStudyParams {
        database_id: body.database_id,
        name: body.name,
        global: body.global,
        start_fen,
        spine,
        spine_config: body.spine,
        model: body.model,
    };
    let movetime_ms = body.movetime_ms.unwrap_or(DEFAULT_MOVETIME_MS);
    let multipv = body.multipv.unwrap_or(DEFAULT_MULTIPV);
    let reports = PositionReportService::new(state.db.clone());

    let outcome = generate_danger_study_live(
        engine,
        &reports,
        provider.as_ref(),
        &StudyService::new(state.db.clone()),
        &user,
        &params,
        movetime_ms,
        multipv,
    )
    .await
    .map_err(|e| {
        let status =
            StatusCode::from_u16(e.http_status_hint()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
        error_response(status, e.client_message())
    })?;

    Ok((StatusCode::CREATED, Json(DangerMapView::from(&outcome))).into_response())
}

/// Body for `POST /api/studies/danger-map`. Mirrors the MCP `danger_map` tool: only
/// the repertoire spine is required; the start position, walk shape and engine
/// budget all default. No `database_id` / `name` — this route returns data, it does
/// not persist a study.
#[derive(Deserialize)]
struct DangerWalkBody {
    /// The repertoire spine as PGN movetext to walk for danger.
    spine_pgn: String,
    /// FEN the walk starts from; defaults to the standard start position.
    #[serde(default)]
    fen: Option<String>,
    /// Walk shape + classifier thresholds; partial overrides over the defaults.
    #[serde(default)]
    spine: SpineConfig,
    /// Per-variation engine movetime budget (ms); capped server-side.
    #[serde(default)]
    movetime_ms: Option<u64>,
    /// MultiPV line count (floored at 2 server-side).
    #[serde(default)]
    multipv: Option<u16>,
}

/// Response for `POST /api/studies/danger-map`: the full engine-adjudicated
/// [`DangerTree`] plus a flat digest of the tagged nodes (most dangerous lines
/// first by walk order), so the client can render arrows + a panel directly.
#[derive(Serialize)]
struct DangerWalkView {
    tree: DangerTree,
    roles: Vec<RoleView>,
}

/// Walk a repertoire spine for dangerous opponent replies and return the raw
/// engine-adjudicated danger tree (`POST /api/studies/danger-map`). Engine only —
/// **no language model** on the path — so the danger overlay works on a local /
/// no-key install. Thin caller over [`walk_danger_spine_live`], symmetric to the
/// MCP `danger_map` tool (ADR-0027). A missing engine is a clean `503`; a malformed
/// spine PGN is a `400`, validated before any operator config is probed.
async fn danger_map(
    State(state): State<AppState>,
    user: CurrentUser,
    Json(body): Json<DangerWalkBody>,
) -> Result<Response, Response> {
    let start_fen = body
        .fen
        .filter(|fen| !fen.trim().is_empty())
        .unwrap_or_else(|| STARTPOS_FEN.to_string());
    let spine = from_pgn_with_start(&body.spine_pgn, &start_fen)
        .map_err(|e| error_response(StatusCode::BAD_REQUEST, format!("invalid spine PGN: {e}")))?;

    let engine = state.engine_service.as_ref().ok_or_else(|| {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            "No engine configured: start chess-base with --engine / CHESS_BASE_ENGINE.",
        )
            .into_response()
    })?;

    let movetime_ms = body.movetime_ms.unwrap_or(DEFAULT_MOVETIME_MS);
    let multipv = body.multipv.unwrap_or(DEFAULT_MULTIPV);
    let reports = PositionReportService::new(state.db.clone());

    let tree = walk_danger_spine_live(
        engine,
        &reports,
        &user,
        &spine,
        &start_fen,
        &body.spine,
        MODE,
        movetime_ms,
        multipv,
    )
    .await
    .map_err(spine_error_response)?;

    let roles = roles_digest(&tree);
    Ok((StatusCode::OK, Json(DangerWalkView { tree, roles })).into_response())
}

/// Flatten a [`DangerTree`] to its tagged nodes (walk order = shallow, most
/// dangerous lines first), mirroring the MCP tool's `roles` digest. The full raw
/// figures (trap verdict, only-move gap, miss-rate, attack) stay embedded per node
/// in the returned `tree`.
fn roles_digest(tree: &DangerTree) -> Vec<RoleView> {
    tree.nodes
        .iter()
        .filter_map(|n| {
            n.tag.as_ref().map(|tag| RoleView {
                node_id: n.id,
                san: n.san.clone(),
                kind: format!("{:?}", tag.kind),
                role: format!("{:?}", tag.role),
                eval: tag.eval,
            })
        })
        .collect()
}

/// Map a [`SpineError`] onto an HTTP response without leaking engine/DB internals:
/// a bad start FEN is the caller's `400`; an analyzer/source failure is a masked
/// `500` (the [`error_response`] envelope hides 5xx detail).
fn spine_error_response(error: SpineError) -> Response {
    match error {
        SpineError::InvalidFen(msg) => {
            error_response(StatusCode::BAD_REQUEST, format!("invalid FEN: {msg}"))
        }
        SpineError::Source(_) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "could not walk the spine for danger",
        ),
    }
}

#[cfg(test)]
#[path = "danger_route_tests.rs"]
mod tests;
