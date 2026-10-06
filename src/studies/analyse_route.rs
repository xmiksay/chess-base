//! HTTP surface for the "Analyse study" pass (`POST /api/studies/{id}/analyse`,
//! #162/#189, shapes #191): engine evals + move classification, optionally
//! regenerating plan/threat arrows, all under one job budget (ADR-0054). Split
//! out of `routes.rs`, which is over the file-size cap.

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::post,
    Json, Router,
};
use serde::{Deserialize, Serialize};

use crate::db::entities::studies;
use crate::engine::{budget, EngineService, Limits};
use crate::server::identity::CurrentUser;
use crate::server::state::AppState;
use crate::studies::routes::StudyView;
use crate::studies::{AnalyseStats, StudyService};
use crate::study_gen::spine::MultiAnalyzer;
use crate::study_gen::{EnginePlanAnalyzer, ShapeConfig, MAX_PLAN_LINES};

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/api/studies/{id}/analyse", post(analyse))
        .with_state(state)
}

/// Per-position engine search depth used by `POST /api/studies/{id}/analyse`
/// (issue #162) when the request doesn't override it. Capped server-side via
/// [`Limits::clamped`].
const DEFAULT_ANALYSE_DEPTH: u32 = 18;

/// Body for `POST /api/studies/{id}/analyse` — the non-destructive "Analyse
/// study" pass (#162, full classification #189). Optional `depth` overrides the
/// per-position engine search depth; everything else is taken from the stored
/// tree. `plan_lines`/`threats` additionally regenerate plan/threat arrows in
/// the same call (issue #191, ADR-0039 addendum): sending either field — even
/// `{plan_lines: 0, threats: false}` — opts in and strips any node's stale
/// generated arrows it doesn't replace; omitting both leaves existing shapes
/// untouched, matching the pre-#191 behavior.
#[derive(Deserialize, Default)]
struct AnalyseBody {
    /// Per-position engine search depth (plies); capped server-side.
    #[serde(default)]
    depth: Option<u32>,
    /// Pin engine "plan" arrows (top-N PV trajectories) on every node; capped
    /// at [`MAX_PLAN_LINES`]. See [`crate::study_gen::plan_shapes`].
    #[serde(default)]
    plan_lines: Option<u8>,
    /// Pin the static "threats" (hanging-piece) arrows on every node.
    #[serde(default)]
    threats: Option<bool>,
}

/// The response of `POST /api/studies/{id}/analyse`: the refreshed study plus
/// the classification roll-up (issue #189), so the editor can render both from
/// one response.
#[derive(Serialize)]
struct AnalyseView {
    #[serde(flatten)]
    study: StudyView,
    stats: AnalyseStats,
}

/// Fill `[%eval]` on every non-terminal node of a study and classify each move
/// (`review::classify`) — a `!`/`?!`/`?`/`??` NAG replaces any prior one; user
/// comments, shapes and positional NAGs are left alone (`POST
/// /api/studies/{id}/analyse`, #162, #189). Mirrors `generate`'s
/// engine-from-state 503 guard.
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

    let body = body.map(|Json(b)| b).unwrap_or_default();
    let depth = body.depth.unwrap_or(DEFAULT_ANALYSE_DEPTH);
    let svc = StudyService::new(state.db.clone());
    // One job budget (ADR-0054) spans the analysis and the shape regeneration.
    let (result, truncated) = budget::job(analyse_job(&svc, engine, &user, id, depth, &body)).await;
    let (model, mut stats) = result?;
    stats.truncated |= truncated;

    let study = StudyView::try_from(model).map_err(IntoResponse::into_response)?;
    Ok((StatusCode::OK, Json(AnalyseView { study, stats })).into_response())
}

async fn analyse_job(
    svc: &StudyService,
    engine: &EngineService,
    user: &CurrentUser,
    id: i32,
    depth: u32,
    body: &AnalyseBody,
) -> Result<(studies::Model, AnalyseStats), Response> {
    let (model, stats) = svc
        .analyse_study(engine, user, id, depth)
        .await
        .map_err(IntoResponse::into_response)?;

    // Optionally regenerate plan/threat arrows in the same call (issue #191):
    // opt in by sending either field, even `{plan_lines: 0, threats: false}` to
    // strip the study's generated arrows without an engine detour.
    let model = if body.plan_lines.is_some() || body.threats.is_some() {
        let cfg = ShapeConfig {
            plan_lines: body.plan_lines.unwrap_or(0).min(MAX_PLAN_LINES),
            threats: body.threats.unwrap_or(false),
        };
        let analyzer = (cfg.plan_lines > 0).then(|| {
            EnginePlanAnalyzer::new(
                engine,
                Limits::depth(depth).clamped(),
                cfg.plan_lines as u16,
            )
        });
        let plans = analyzer.as_ref().map(|a| a as &(dyn MultiAnalyzer + Sync));
        svc.regenerate_shapes(plans, user, id, &cfg)
            .await
            .map_err(IntoResponse::into_response)?
    } else {
        model
    };
    Ok((model, stats))
}
