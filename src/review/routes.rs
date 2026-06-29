//! HTTP surface for the full-game engine review (Mode A, issue #119):
//! `POST /api/games/{id}/analyse?depth=` replays a stored game and runs the
//! engine over every ply, returning per-move classifications + explanations and
//! a per-side accuracy summary. A thin caller of [`review_game`]: it loads the
//! game (scoped to the caller via [`GameService`]), parses its mainline, and
//! delegates all analysis to the service.

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::post,
    Json, Router,
};
use serde::Deserialize;

use crate::games::{GameError, GameService};
use crate::ingest::parse_pgn;
use crate::position::STARTPOS_FEN;
use crate::review::review_game;
use crate::server::error::error_response;
use crate::server::identity::CurrentUser;
use crate::server::state::AppState;

use super::service::ReviewError;

/// Default per-ply search depth: shallow enough for a fast whole-game pass, deep
/// enough to classify moves reliably. Clamped server-side by [`review_game`].
const DEFAULT_REVIEW_DEPTH: u32 = 16;

/// Review routes, mounted under the main API router.
pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/api/games/{id}/analyse", post(analyse))
        .with_state(state)
}

/// `?depth=<plies>` — per-position search depth (optional; capped server-side).
#[derive(Deserialize)]
struct AnalyseQuery {
    #[serde(default)]
    depth: Option<u32>,
}

/// `POST /api/games/{id}/analyse` — engine-only full-game review.
async fn analyse(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<i32>,
    Query(q): Query<AnalyseQuery>,
) -> Result<Response, Response> {
    // A missing engine is an operator-configuration gap, not a leaked internal —
    // surface the guidance verbatim (mirrors the analysis WS / study generate).
    let engine = state.engine_service.as_ref().ok_or_else(|| {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            "No engine configured: start chess-base with --engine / CHESS_BASE_ENGINE.",
        )
            .into_response()
    })?;

    let game = GameService::new(state.db.clone())
        .get(&user, id)
        .await
        .map_err(game_error_response)?;
    let pgn = game
        .pgn
        .as_deref()
        .ok_or_else(|| review_error_response(ReviewError::BadGame("game has no moves".into())))?;
    let parsed =
        parse_pgn(pgn).map_err(|e| review_error_response(ReviewError::BadGame(e.to_string())))?;

    let start_fen = game.start_fen.as_deref().unwrap_or(STARTPOS_FEN);
    let depth = q.depth.unwrap_or(DEFAULT_REVIEW_DEPTH);
    let review = review_game(engine, start_fen, &game.variant, &parsed.mainline, depth)
        .await
        .map_err(review_error_response)?;

    Ok((StatusCode::OK, Json(review)).into_response())
}

/// Map a game-lookup failure onto an HTTP response (not-found hides ids the
/// caller can't see; storage errors stay generic).
fn game_error_response(err: GameError) -> Response {
    let status = match err {
        GameError::NotFound => StatusCode::NOT_FOUND,
        GameError::Db(_) => StatusCode::INTERNAL_SERVER_ERROR,
    };
    error_response(status, err.to_string())
}

/// Map a review failure onto an HTTP response: a bad game is a client error with
/// a safe message; an engine failure is masked as a generic 5xx.
fn review_error_response(err: ReviewError) -> Response {
    match err {
        ReviewError::BadGame(msg) => error_response(StatusCode::UNPROCESSABLE_ENTITY, msg),
        ReviewError::Engine(_) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "engine analysis failed".to_string(),
        ),
    }
}
