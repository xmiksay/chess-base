//! HTTP surface for game listing (issue #68): a keyset-paginated list of the
//! games in a database, and a single-game fetch (with PGN) for board playback.
//! Plus extended-PGN export (issue #120): a real `.pgn` download, verbatim or
//! with the #119 engine analysis embedded as `[%eval]` + NAGs + why-notes.
//! Thin callers of [`GameService`]; all scoping lives in the service.

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};

use crate::db::entities::studies;
use crate::games::{
    export, GameError, GameListParams, GameService, GameSort, SortDir, DEFAULT_LIMIT,
};
use crate::ingest::parse_pgn;
use crate::pgn_tree::pgn::from_pgn_with_start;
use crate::position::STARTPOS_FEN;
use crate::review::{review_game, ReviewError};
use crate::server::download::pgn_attachment;
use crate::server::error::error_response;
use crate::server::identity::CurrentUser;
use crate::server::state::AppState;
use crate::studies::{StudyError, StudyService};

/// Default per-ply search depth for an annotated export, mirroring the review
/// route (issue #119). Clamped server-side by [`review_game`].
const DEFAULT_EXPORT_DEPTH: u32 = 16;

/// Default per-position search depth for "save as analysis" with `analyse=true`
/// (issue #164), mirroring the study analyse route (#162). Clamped server-side.
const DEFAULT_SAVE_ANALYSE_DEPTH: u32 = 18;

/// Game routes, mounted under the main API router.
pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/api/games", get(list))
        .route("/api/games/{id}", get(get_one).delete(delete_one))
        .route("/api/games/{id}/tree", get(get_tree))
        .route("/api/games/{id}/export", get(export_pgn))
        .route("/api/games/{id}/save-as-study", post(save_as_study))
        .route("/api/games/{id}/studies", get(linked_studies))
        .with_state(state)
}

/// `?database_id=<id>&page=<n>&limit=<n>&sort=<field>&dir=<asc|desc>` for the list
/// endpoint. `page` is 0-based; `sort`/`dir` default to date, newest-first; both
/// `limit` and unknown sort/dir values are normalised by the service.
#[derive(Deserialize)]
struct ListQuery {
    database_id: i32,
    #[serde(default)]
    page: Option<u64>,
    #[serde(default)]
    limit: Option<u64>,
    #[serde(default)]
    sort: Option<String>,
    #[serde(default)]
    dir: Option<String>,
}

/// `GET /api/games?database_id=…&page=…&limit=…&sort=…&dir=…` — one sorted,
/// offset-paginated page of games (with the total count).
async fn list(
    State(state): State<AppState>,
    user: CurrentUser,
    Query(q): Query<ListQuery>,
) -> Result<Response, GameError> {
    let params = GameListParams {
        database_id: q.database_id,
        page: q.page.unwrap_or(0),
        limit: q.limit.unwrap_or(DEFAULT_LIMIT),
        sort: GameSort::parse(q.sort.as_deref()),
        dir: SortDir::parse(q.dir.as_deref()),
    };
    let page = service(&state).list(&user, &params).await?;
    Ok((StatusCode::OK, Json(page)).into_response())
}

/// `GET /api/games/{id}` — a single game with its PGN movetext.
async fn get_one(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<i32>,
) -> Result<Response, GameError> {
    let game = service(&state).get(&user, id).await?;
    Ok((StatusCode::OK, Json(game)).into_response())
}

/// `DELETE /api/games/{id}` — remove a game the caller may write (owner, or admin
/// for a global database). Returns `204 No Content`; a hidden/absent id is `404`,
/// a visible-but-unwritable database (global, non-admin) is `403`.
async fn delete_one(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<i32>,
) -> Result<Response, GameError> {
    service(&state).delete(&user, id).await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

/// `GET /api/games/{id}/tree` — the stored game parsed into a [`MoveTree`],
/// preserving the `(…)` sub-variations the frontend's chess.js flattener drops.
/// Thin caller of [`from_pgn_with_start`]; the game's `start_fen` (a SetUp `[FEN]`
/// origin) is threaded in so non-standard starts round-trip. A parse / illegal-
/// move failure maps to a clean 422 — no raw `DbErr` or panic leaks.
async fn get_tree(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<i32>,
) -> Result<Response, Response> {
    let game = service(&state)
        .get(&user, id)
        .await
        .map_err(game_error_response)?;
    let pgn = game.pgn.as_deref().unwrap_or_default();
    let start_fen = game.start_fen.as_deref().unwrap_or(STARTPOS_FEN);
    let tree = from_pgn_with_start(pgn, start_fen)
        .map_err(|e| error_response(StatusCode::UNPROCESSABLE_ENTITY, e.to_string()))?;
    Ok((StatusCode::OK, Json(tree)).into_response())
}

/// `?annotated=<bool>&depth=<n>` for the export endpoint. `annotated=false` (the
/// default) downloads the stored PGN verbatim; `annotated=true` runs the #119
/// review and embeds `[%eval]` + NAGs + why-notes (engine required).
#[derive(Deserialize)]
struct ExportQuery {
    #[serde(default)]
    annotated: bool,
    #[serde(default)]
    depth: Option<u32>,
}

/// `GET /api/games/{id}/export` — download a game as a real `.pgn` file. Returns
/// an `Err(Response)` for engine/analysis failures (mirrors the review route) so
/// a missing engine surfaces its operator guidance verbatim.
async fn export_pgn(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<i32>,
    Query(q): Query<ExportQuery>,
) -> Result<Response, Response> {
    let game = service(&state)
        .get(&user, id)
        .await
        .map_err(game_error_response)?;

    let pgn = if q.annotated {
        let engine = state.engine_service.as_ref().ok_or_else(|| {
            (
                StatusCode::SERVICE_UNAVAILABLE,
                "No engine configured: start chess-base with --engine / CHESS_BASE_ENGINE.",
            )
                .into_response()
        })?;
        let stored = game.pgn.as_deref().ok_or_else(|| {
            review_error_response(ReviewError::BadGame("game has no moves".into()))
        })?;
        let parsed = parse_pgn(stored)
            .map_err(|e| review_error_response(ReviewError::BadGame(e.to_string())))?;
        let start_fen = game.start_fen.as_deref().unwrap_or(STARTPOS_FEN);
        let depth = q.depth.unwrap_or(DEFAULT_EXPORT_DEPTH);
        let review = review_game(engine, start_fen, &game.variant, &parsed.mainline, depth)
            .await
            .map_err(review_error_response)?;
        let tree = export::annotated_tree(&parsed.mainline, &review);
        export::to_annotated_pgn(&game, &tree)
            .map_err(|e| review_error_response(ReviewError::BadGame(e.to_string())))?
    } else {
        // Verbatim download of the stored game (no engine needed).
        game.pgn.clone().unwrap_or_default()
    };

    Ok(pgn_attachment(&format!("game-{id}.pgn"), pgn))
}

/// Study metadata returned by the game↔analysis endpoints — the same shape the
/// studies list uses, so the SPA can reuse its `StudySummary` type.
#[derive(Serialize)]
struct StudyLink {
    id: i32,
    database_id: i32,
    owner_id: Option<String>,
    name: String,
    global: bool,
    folder_id: Option<i32>,
    origin_game_id: Option<i32>,
}

impl From<studies::Model> for StudyLink {
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

/// Body for `POST /api/games/{id}/save-as-study`: persist the game as an analysis
/// (a study linked via `origin_game_id`), optionally running the engine review.
#[derive(Deserialize)]
struct SaveAsStudyBody {
    name: String,
    #[serde(default)]
    folder_id: Option<i32>,
    /// Run the #119 engine review and embed `[%eval]`/NAGs/why-notes. Defaults off.
    #[serde(default)]
    analyse: bool,
    /// Per-position engine search depth when `analyse` is set; capped server-side.
    #[serde(default)]
    depth: Option<u32>,
}

/// `POST /api/games/{id}/save-as-study` — create an analysis from a game. Returns
/// `Err(Response)` so a missing-engine config (with `analyse=true`) surfaces its
/// operator guidance verbatim, mirroring the study analyse route.
async fn save_as_study(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<i32>,
    Json(body): Json<SaveAsStudyBody>,
) -> Result<Response, Response> {
    let engine = if body.analyse {
        let engine = state.engine_service.as_ref().ok_or_else(|| {
            (
                StatusCode::SERVICE_UNAVAILABLE,
                "No engine configured: start chess-base with --engine / CHESS_BASE_ENGINE.",
            )
                .into_response()
        })?;
        Some(engine.as_ref())
    } else {
        None
    };
    let depth = body.depth.unwrap_or(DEFAULT_SAVE_ANALYSE_DEPTH);
    let model = StudyService::new(state.db.clone())
        .create_from_game(
            engine,
            &user,
            id,
            body.name,
            body.folder_id,
            body.analyse,
            depth,
        )
        .await
        .map_err(IntoResponse::into_response)?;
    Ok((StatusCode::CREATED, Json(StudyLink::from(model))).into_response())
}

/// `GET /api/games/{id}/studies` — the analyses linked to this game.
async fn linked_studies(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<i32>,
) -> Result<Response, StudyError> {
    let rows = StudyService::new(state.db.clone())
        .studies_for_game(&user, id)
        .await?;
    let views: Vec<StudyLink> = rows.into_iter().map(StudyLink::from).collect();
    Ok((StatusCode::OK, Json(views)).into_response())
}

/// Map a game-lookup failure onto a response (not-found hides ids the caller
/// can't see; storage errors stay generic).
fn game_error_response(err: GameError) -> Response {
    let status = match err {
        GameError::NotFound => StatusCode::NOT_FOUND,
        GameError::Forbidden => StatusCode::FORBIDDEN,
        GameError::Db(_) => StatusCode::INTERNAL_SERVER_ERROR,
    };
    error_response(status, err.to_string())
}

/// Map a review failure onto a response: a bad game is a client error with a
/// safe message; an engine failure is masked as a generic 5xx.
fn review_error_response(err: ReviewError) -> Response {
    match err {
        ReviewError::BadGame(msg) => error_response(StatusCode::UNPROCESSABLE_ENTITY, msg),
        ReviewError::Engine(_) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "engine analysis failed".to_string(),
        ),
    }
}

fn service(state: &AppState) -> GameService {
    GameService::new(state.db.clone())
}

/// Map service failures onto HTTP status + a JSON error envelope. 5xx details are
/// internal; clients get a generic message (never a raw `DbErr`).
impl IntoResponse for GameError {
    fn into_response(self) -> Response {
        let status = match &self {
            GameError::NotFound => StatusCode::NOT_FOUND,
            GameError::Forbidden => StatusCode::FORBIDDEN,
            GameError::Db(_) => StatusCode::INTERNAL_SERVER_ERROR,
        };
        error_response(status, self.to_string())
    }
}
