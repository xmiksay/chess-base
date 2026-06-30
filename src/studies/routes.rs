//! HTTP surface for studies: lifecycle CRUD + PGN import/export (issue #9) and
//! node mutation — add SAN-validated moves/variations, annotate (comment/NAG),
//! promote / reorder / delete variations (issue #18). Thin callers of
//! [`StudyService`] that translate JSON ⇄ models; all SAN validation and
//! ownership/admin gating lives in the service (so the Epic 9 batch annotation
//! pass reuses the same code, per ADR-0009).

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post, put},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::db::entities::studies;
use crate::engine::Limits;
use crate::pgn_tree::pgn::PgnError;
use crate::pgn_tree::{MoveTree, Shape};
use crate::position::STARTPOS_FEN;
use crate::search::report::PositionReportService;
use crate::server::download::pgn_attachment;
use crate::server::error::error_response;
use crate::server::identity::CurrentUser;
use crate::server::state::AppState;
use crate::studies::{StudyError, StudyService};
use crate::study_gen::tree::TreeConfig;
use crate::study_gen::{
    generate_study_live, GenerateError, GenerateOutcome, GenerateParams, MAX_PLAN_LINES,
};

/// Per-position engine search depth used by `POST /api/studies/generate` when the
/// request doesn't override it. Moderate so a generated tree's ground-truth evals
/// land quickly; capped server-side via [`Limits::clamped`].
const DEFAULT_GENERATE_DEPTH: u32 = 18;

/// Per-position engine search depth used by `POST /api/studies/{id}/analyse`
/// (issue #162) when the request doesn't override it. Capped server-side via
/// [`Limits::clamped`].
const DEFAULT_ANALYSE_DEPTH: u32 = 18;

/// Study routes, mounted under the main API router.
pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/api/studies", get(list).post(create))
        .route("/api/studies/import", post(import))
        .route("/api/studies/generate", post(generate))
        .route(
            "/api/studies/{id}",
            get(get_one).patch(rename).delete(delete),
        )
        .route("/api/studies/{id}/folder", put(set_folder))
        .route("/api/studies/{id}/export", get(export))
        .route("/api/studies/{id}/export/lichess", get(export_lichess))
        .route("/api/studies/{id}/analyse", post(analyse))
        .route("/api/studies/{id}/moves", post(add_move))
        .route(
            "/api/studies/{id}/nodes/{node_id}",
            axum::routing::delete(delete_node),
        )
        .route("/api/studies/{id}/nodes/{node_id}/annotate", post(annotate))
        .route("/api/studies/{id}/nodes/{node_id}/shapes", put(set_shapes))
        .route("/api/studies/{id}/nodes/{node_id}/promote", post(promote))
        .route("/api/studies/{id}/nodes/{node_id}/reorder", post(reorder))
        .with_state(state)
}

/// Lightweight study metadata for listings (no move tree).
#[derive(Serialize)]
struct StudySummary {
    id: i32,
    database_id: i32,
    owner_id: Option<String>,
    name: String,
    /// Convenience flag for the SPA: `owner_id IS NULL`.
    global: bool,
    /// Which folder the study is filed in (issue #164); `None` ⇒ unfiled/root.
    folder_id: Option<i32>,
    /// The game an analysis was built from (issue #164); `None` ⇒ standalone.
    origin_game_id: Option<i32>,
}

impl From<studies::Model> for StudySummary {
    fn from(m: studies::Model) -> Self {
        Self {
            global: m.owner_id.is_none(),
            id: m.id,
            database_id: m.database_id,
            owner_id: m.owner_id,
            name: m.name,
            folder_id: m.folder_id,
            origin_game_id: m.origin_game_id,
        }
    }
}

/// A study with its full parsed move tree, returned by `get` and every mutation
/// so the editor can re-render without a second request.
#[derive(Serialize)]
struct StudyView {
    #[serde(flatten)]
    summary: StudySummary,
    tree: MoveTree,
}

impl TryFrom<studies::Model> for StudyView {
    type Error = StudyError;

    fn try_from(m: studies::Model) -> Result<Self, Self::Error> {
        let tree: MoveTree = serde_json::from_str(&m.tree_json)?;
        Ok(Self {
            summary: StudySummary::from(m),
            tree,
        })
    }
}

#[derive(Deserialize)]
struct CreateBody {
    database_id: i32,
    name: String,
    /// Create a global (admin-owned) study; requires admin. Defaults to false.
    #[serde(default)]
    global: bool,
}

#[derive(Deserialize)]
struct RenameBody {
    name: String,
}

#[derive(Deserialize)]
struct ImportBody {
    database_id: i32,
    name: String,
    /// PGN movetext (first game) to parse into the new study's tree.
    pgn: String,
    /// Create a global (admin-owned) study; requires admin. Defaults to false.
    #[serde(default)]
    global: bool,
}

/// Body for `POST /api/studies/generate` — the AI-assisted study-generation
/// orchestrator (#115). Only the target database and name are required; the start
/// position defaults to the standard opening and pruning to [`TreeConfig::default`].
#[derive(Deserialize)]
struct GenerateBody {
    database_id: i32,
    name: String,
    /// Create a global (admin-owned) study; requires admin. Defaults to false.
    #[serde(default)]
    global: bool,
    /// FEN to grow the study from; defaults to the standard start position.
    #[serde(default)]
    start_fen: Option<String>,
    /// LLM model id; defaults to the provider's default model.
    #[serde(default)]
    model: Option<String>,
    /// Tree pruning thresholds; defaults to [`TreeConfig::default`].
    #[serde(default)]
    tree: Option<TreeConfig>,
    /// Per-position engine search depth (plies); capped server-side.
    #[serde(default)]
    engine_depth: Option<u32>,
    /// Pin engine "plan" arrows (top-N PV trajectories) on every node; 0/omitted
    /// = off, capped at [`MAX_PLAN_LINES`]. See [`crate::study_gen::plan_shapes`].
    #[serde(default)]
    plan_lines: Option<u8>,
    /// Pin the static "threats" (hanging-piece) arrows on every node.
    #[serde(default)]
    threats: Option<bool>,
}

/// Summary returned by `generate`: the created study plus what the verification
/// loop dropped, so the caller can fetch the full tree via `GET /api/studies/{id}`.
#[derive(Serialize)]
struct GenerateView {
    id: i32,
    database_id: i32,
    name: String,
    global: bool,
    /// Nodes in the committed annotated tree.
    node_count: usize,
    /// How many model claims/glyphs ground truth rejected (never committed).
    rejected: usize,
}

impl From<&GenerateOutcome> for GenerateView {
    fn from(outcome: &GenerateOutcome) -> Self {
        Self {
            id: outcome.study.id,
            database_id: outcome.study.database_id,
            name: outcome.study.name.clone(),
            global: outcome.study.owner_id.is_none(),
            node_count: outcome.node_count,
            rejected: outcome.rejected.len(),
        }
    }
}

#[derive(Deserialize)]
struct AddMoveBody {
    /// Node to branch from; the move becomes a child of it (a variation when the
    /// node already has children).
    from_node_id: usize,
    san: String,
}

#[derive(Deserialize)]
struct AnnotateBody {
    #[serde(default)]
    comment: Option<String>,
    #[serde(default)]
    nag: Option<u8>,
}

#[derive(Deserialize)]
struct ReorderBody {
    /// Target position among siblings (0 = mainline).
    index: usize,
}

#[derive(Deserialize)]
struct ShapesBody {
    /// Board shapes to pin to the node; an empty list clears the pin.
    shapes: Vec<Shape>,
}

async fn list(State(state): State<AppState>, user: CurrentUser) -> Result<Response, StudyError> {
    let rows = service(&state).list(&user).await?;
    let views: Vec<StudySummary> = rows.into_iter().map(StudySummary::from).collect();
    Ok((StatusCode::OK, Json(views)).into_response())
}

async fn create(
    State(state): State<AppState>,
    user: CurrentUser,
    Json(body): Json<CreateBody>,
) -> Result<Response, StudyError> {
    let model = service(&state)
        .create(&user, body.database_id, body.name, body.global)
        .await?;
    Ok((StatusCode::CREATED, Json(StudyView::try_from(model)?)).into_response())
}

/// Import a PGN game as a new study (`POST /api/studies/import`).
async fn import(
    State(state): State<AppState>,
    user: CurrentUser,
    Json(body): Json<ImportBody>,
) -> Result<Response, StudyError> {
    let model = service(&state)
        .import_pgn(&user, body.database_id, body.name, &body.pgn, body.global)
        .await?;
    Ok((StatusCode::CREATED, Json(StudyView::try_from(model)?)).into_response())
}

/// Generate an annotated study from a start position (`POST /api/studies/generate`).
/// Thin caller over [`generate_study_live`]: it runs the tree builder → batch LLM
/// annotation + verification pass → persist, all scoped to the caller. Requires
/// both an engine and an LLM provider configured; failures surface a clean status
/// without leaking engine/DB/LLM internals.
async fn generate(
    State(state): State<AppState>,
    user: CurrentUser,
    Json(body): Json<GenerateBody>,
) -> Result<Response, Response> {
    // A missing engine / model is an operator-configuration gap, not a leaked
    // internal — surface the guidance verbatim (like the analysis WS), rather than
    // through the 5xx-masking `error_response`.
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

    let params = GenerateParams {
        database_id: body.database_id,
        name: body.name,
        global: body.global,
        start_fen: body.start_fen.unwrap_or_else(|| STARTPOS_FEN.to_string()),
        tree: body.tree.unwrap_or_default(),
        model: body.model,
        plan_lines: body.plan_lines.unwrap_or(0).min(MAX_PLAN_LINES),
        threats: body.threats.unwrap_or(false),
    };
    let limits = Limits::depth(body.engine_depth.unwrap_or(DEFAULT_GENERATE_DEPTH)).clamped();
    let reports = PositionReportService::new(state.db.clone());

    let outcome = generate_study_live(
        engine,
        &reports,
        provider.as_ref(),
        &service(&state),
        &user,
        &params,
        limits,
    )
    .await
    .map_err(generate_error_response)?;

    Ok((StatusCode::CREATED, Json(GenerateView::from(&outcome))).into_response())
}

/// Map a [`GenerateError`] onto an HTTP response using its transport-agnostic
/// status hint + client-safe message (never a raw `DbErr`, engine or LLM detail).
fn generate_error_response(error: GenerateError) -> Response {
    let status =
        StatusCode::from_u16(error.http_status_hint()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    error_response(status, error.client_message())
}

async fn get_one(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<i32>,
) -> Result<Response, StudyError> {
    let model = service(&state).get(&user, id).await?;
    Ok((StatusCode::OK, Json(StudyView::try_from(model)?)).into_response())
}

/// `?eval=<bool>` for the study export: `eval=true` (the default) keeps the
/// per-move `[%eval]` annotations (the extended export, issue #120); `eval=false`
/// strips them for a plain export. Comments / NAGs / shapes are always kept.
#[derive(Deserialize)]
struct ExportQuery {
    #[serde(default = "default_true")]
    eval: bool,
}

fn default_true() -> bool {
    true
}

/// Export a study as a downloadable `.pgn` movetext file
/// (`GET /api/studies/{id}/export?eval=<bool>`).
async fn export(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<i32>,
    Query(q): Query<ExportQuery>,
) -> Result<Response, StudyError> {
    let pgn = service(&state).export_pgn(&user, id, q.eval).await?;
    Ok(pgn_attachment(&format!("study-{id}.pgn"), pgn))
}

/// Export a study as a downloadable Lichess-study chapter — PGN with header tags
/// (`GET /api/studies/{id}/export/lichess`). Always carries the extended
/// annotations (`[%eval]`/NAGs/comments/shapes).
async fn export_lichess(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<i32>,
) -> Result<Response, StudyError> {
    let pgn = service(&state).export_lichess(&user, id).await?;
    Ok(pgn_attachment(&format!("study-{id}-lichess.pgn"), pgn))
}

/// Body for `POST /api/studies/{id}/analyse` — the non-destructive "Analyse
/// study" pass (#162). Optional `depth` overrides the per-position engine search
/// depth; everything else is taken from the stored tree.
#[derive(Deserialize, Default)]
struct AnalyseBody {
    /// Per-position engine search depth (plies); capped server-side.
    #[serde(default)]
    depth: Option<u32>,
}

/// Fill `[%eval]` on every non-terminal node of a study and persist it
/// (`POST /api/studies/{id}/analyse`, #162). Eval-only — comments / NAGs / shapes
/// stay put. Mirrors `generate`'s engine-from-state 503 guard; returns the
/// refreshed `StudyView` so the editor re-renders from one response.
async fn analyse(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<i32>,
    body: Option<Json<AnalyseBody>>,
) -> Result<Response, Response> {
    // A missing engine is an operator-configuration gap, not a leaked internal —
    // surface the guidance verbatim (like `generate`), not a 5xx.
    let engine = state.engine_service.as_ref().ok_or_else(|| {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            "No engine configured: start chess-base with --engine / CHESS_BASE_ENGINE.",
        )
            .into_response()
    })?;

    let depth = body
        .and_then(|Json(b)| b.depth)
        .unwrap_or(DEFAULT_ANALYSE_DEPTH);
    let model = service(&state)
        .analyse_study(engine, &user, id, depth)
        .await
        .map_err(IntoResponse::into_response)?;
    let view = StudyView::try_from(model).map_err(IntoResponse::into_response)?;
    Ok((StatusCode::OK, Json(view)).into_response())
}

async fn rename(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<i32>,
    Json(body): Json<RenameBody>,
) -> Result<Response, StudyError> {
    let model = service(&state).rename(&user, id, body.name).await?;
    Ok((StatusCode::OK, Json(StudyView::try_from(model)?)).into_response())
}

/// Body for `PUT /api/studies/{id}/folder`: the target folder, or `null` to unfile
/// the study (move it to the root).
#[derive(Deserialize)]
struct FolderBody {
    #[serde(default)]
    folder_id: Option<i32>,
}

/// Move a study into a folder (`PUT /api/studies/{id}/folder`). Returns the
/// refreshed view so the editor and sidebar re-render from one response.
async fn set_folder(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<i32>,
    Json(body): Json<FolderBody>,
) -> Result<Response, StudyError> {
    let model = service(&state)
        .set_folder(&user, id, body.folder_id)
        .await?;
    Ok((StatusCode::OK, Json(StudyView::try_from(model)?)).into_response())
}

async fn delete(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<i32>,
) -> Result<Response, StudyError> {
    service(&state).delete(&user, id).await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

async fn add_move(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<i32>,
    Json(body): Json<AddMoveBody>,
) -> Result<Response, StudyError> {
    let svc = service(&state);
    let new_node_id = svc
        .add_move(&user, id, body.from_node_id, &body.san)
        .await?;
    // Return the new node id alongside the refreshed tree.
    let view = StudyView::try_from(svc.get(&user, id).await?)?;
    Ok((
        StatusCode::CREATED,
        Json(json!({ "new_node_id": new_node_id, "study": view })),
    )
        .into_response())
}

async fn annotate(
    State(state): State<AppState>,
    user: CurrentUser,
    Path((id, node_id)): Path<(i32, usize)>,
    Json(body): Json<AnnotateBody>,
) -> Result<Response, StudyError> {
    let svc = service(&state);
    svc.annotate(&user, id, node_id, body.comment, body.nag)
        .await?;
    refreshed(&svc, &user, id).await
}

/// Pin (or clear, with an empty list) a node's board shapes
/// (`PUT /api/studies/{id}/nodes/{node_id}/shapes`).
async fn set_shapes(
    State(state): State<AppState>,
    user: CurrentUser,
    Path((id, node_id)): Path<(i32, usize)>,
    Json(body): Json<ShapesBody>,
) -> Result<Response, StudyError> {
    let svc = service(&state);
    svc.set_shapes(&user, id, node_id, body.shapes).await?;
    refreshed(&svc, &user, id).await
}

async fn promote(
    State(state): State<AppState>,
    user: CurrentUser,
    Path((id, node_id)): Path<(i32, usize)>,
) -> Result<Response, StudyError> {
    let svc = service(&state);
    svc.promote_variation(&user, id, node_id).await?;
    refreshed(&svc, &user, id).await
}

async fn reorder(
    State(state): State<AppState>,
    user: CurrentUser,
    Path((id, node_id)): Path<(i32, usize)>,
    Json(body): Json<ReorderBody>,
) -> Result<Response, StudyError> {
    let svc = service(&state);
    svc.reorder_variation(&user, id, node_id, body.index)
        .await?;
    refreshed(&svc, &user, id).await
}

async fn delete_node(
    State(state): State<AppState>,
    user: CurrentUser,
    Path((id, node_id)): Path<(i32, usize)>,
) -> Result<Response, StudyError> {
    let svc = service(&state);
    svc.delete_node(&user, id, node_id).await?;
    refreshed(&svc, &user, id).await
}

/// Re-read the study and return its refreshed view (200). Shared tail of the
/// mutation handlers so the editor always gets the post-edit tree back.
async fn refreshed(
    svc: &StudyService,
    user: &CurrentUser,
    id: i32,
) -> Result<Response, StudyError> {
    let view = StudyView::try_from(svc.get(user, id).await?)?;
    Ok((StatusCode::OK, Json(view)).into_response())
}

fn service(state: &AppState) -> StudyService {
    StudyService::new(state.db.clone())
}

/// Map service failures onto HTTP status + a JSON error envelope. 5xx details are
/// internal; clients get a generic message (never a raw `DbErr`).
impl IntoResponse for StudyError {
    fn into_response(self) -> Response {
        let status = match &self {
            StudyError::NotFound => StatusCode::NOT_FOUND,
            StudyError::Forbidden => StatusCode::FORBIDDEN,
            StudyError::InvalidNode(_)
            | StudyError::IllegalMove { .. }
            | StudyError::InvalidEdit(_) => StatusCode::BAD_REQUEST,
            // Malformed PGN or an illegal move in submitted PGN is client error;
            // a missing SAN means our own stored tree is corrupt (500).
            StudyError::Pgn(PgnError::MissingSan(_)) => StatusCode::INTERNAL_SERVER_ERROR,
            StudyError::Pgn(_) => StatusCode::BAD_REQUEST,
            StudyError::Tree(_)
            | StudyError::Position(_)
            | StudyError::Db(_)
            | StudyError::Engine(_) => StatusCode::INTERNAL_SERVER_ERROR,
        };
        error_response(status, self.to_string())
    }
}
