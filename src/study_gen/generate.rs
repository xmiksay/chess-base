//! Study-generation orchestrator (issue #115): the one operation that ties the
//! Epic 9 stages into a single, user-invokable "generate an annotated study for
//! this position" call. It is transport-agnostic — the MCP `generate_study` tool
//! and the `POST /api/studies/generate` route are thin callers over it.
//!
//! The pipeline (ADR-0009: engine/DB are ground truth, the LLM only annotates):
//!   1. [`build_tree`] walks the start position into a pruned, feature-tagged
//!      [`VariationTree`] (issues #29/#98 + #30),
//!   2. [`annotate_tree`] runs the batch LLM annotation + verification pass (#31)
//!      — no engine eval/PV ever reaches the model context,
//!   3. the verified [`MoveTree`] is persisted as a [`studies`] row owned by the
//!      caller via [`StudyService`].
//!
//! The core [`generate_study`] is generic over the [`Evaluator`] /
//! [`ContinuationSource`] / [`LlmProvider`] seams so it is unit-testable with
//! injected fakes; [`generate_study_live`] is the production wrapper that builds
//! the real engine + DB adapters.

use crate::ai::llm::LlmProvider;
use crate::db::entities::studies;
use crate::engine::{EngineService, Limits};
use crate::position::CastlingMode;
use crate::search::report::PositionReportService;
use crate::server::identity::CurrentUser;
use crate::studies::{StudyError, StudyService};

use super::plan_shapes::{apply_shapes, ShapeConfig};
use super::spine::MultiAnalyzer;
use super::tree::{
    build_tree, ContinuationSource, Evaluator, TreeConfig, TreeError, VariationTree,
};
use super::{
    annotate_tree, AnnotateError, EngineEvaluator, EnginePlanAnalyzer, Rejection,
    ReportContinuations, MAX_PLAN_LINES,
};

/// Studies are standard chess (mirrors [`crate::studies`]); the generated tree
/// parses castling rights the normal way.
const MODE: CastlingMode = CastlingMode::Standard;

/// What to generate: the start position plus where to file the resulting study.
/// Pruning is governed by [`TreeConfig`]; the LLM `model` defaults to the
/// provider's own default when omitted.
#[derive(Clone, Debug)]
pub struct GenerateParams {
    /// Database the new study belongs to.
    pub database_id: i32,
    /// Name for the new study.
    pub name: String,
    /// Make it a global (admin-owned) study; requires admin.
    pub global: bool,
    /// FEN of the position to grow the study from.
    pub start_fen: String,
    /// Tree builder pruning thresholds (depth/breadth/frequency/eval margin).
    pub tree: TreeConfig,
    /// LLM model id; `None` ⇒ the provider's default model.
    pub model: Option<String>,
    /// Number of engine PV lines to pin as "plan" arrows on every node (0 = off,
    /// capped at [`MAX_PLAN_LINES`]). See [`super::plan_shapes`].
    pub plan_lines: u8,
    /// Pin the static "threats" (hanging-piece) arrows on every node.
    pub threats: bool,
}

/// The persisted study plus a summary of what the verification loop dropped.
#[derive(Clone, Debug)]
pub struct GenerateOutcome {
    /// The newly created, annotated study row.
    pub study: studies::Model,
    /// Number of nodes in the committed move tree.
    pub node_count: usize,
    /// Claims / glyphs the ground-truth verification rejected (never committed).
    pub rejected: Vec<Rejection>,
}

/// Why study generation failed. Transport-agnostic; the HTTP / MCP layer maps
/// [`http_status_hint`](Self::http_status_hint) and [`client_message`](Self::client_message)
/// onto its own envelope so engine/DB/LLM internals never leak to clients.
#[derive(Debug, thiserror::Error)]
pub enum GenerateError {
    /// The tree builder rejected the start position or its DB/engine lookups failed.
    #[error(transparent)]
    Tree(#[from] TreeError),
    /// The annotation pass failed before it could verify and commit anything.
    #[error(transparent)]
    Annotate(#[from] AnnotateError),
    /// Persisting the generated study failed (ownership or database error).
    #[error(transparent)]
    Study(#[from] StudyError),
}

impl GenerateError {
    /// A safe, client-facing message: it names the *stage* that failed without
    /// surfacing a raw `DbErr`, engine output or provider transport detail.
    pub fn client_message(&self) -> String {
        match self {
            GenerateError::Tree(TreeError::InvalidFen(msg)) => format!("invalid FEN: {msg}"),
            GenerateError::Tree(TreeError::Source(_)) => {
                "could not read the position from the database".into()
            }
            GenerateError::Annotate(AnnotateError::Provider(_)) => {
                "the language model request failed".into()
            }
            GenerateError::Annotate(AnnotateError::EmptyResponse)
            | GenerateError::Annotate(AnnotateError::Parse(_)) => {
                "the language model returned an unusable response".into()
            }
            GenerateError::Study(StudyError::Forbidden) => StudyError::Forbidden.to_string(),
            GenerateError::Study(StudyError::Db(_)) => "study could not be saved".into(),
            GenerateError::Study(other) => other.to_string(),
        }
    }

    /// HTTP status hint (raw code) so the route maps failures without this module
    /// depending on `axum`: client mistakes are 4xx, upstream/internal faults 5xx.
    pub fn http_status_hint(&self) -> u16 {
        match self {
            GenerateError::Tree(TreeError::InvalidFen(_)) => 400,
            GenerateError::Tree(TreeError::Source(_)) => 500,
            GenerateError::Annotate(_) => 502,
            GenerateError::Study(StudyError::Forbidden) => 403,
            GenerateError::Study(StudyError::NotFound) => 404,
            GenerateError::Study(StudyError::Db(_)) | GenerateError::Study(StudyError::Tree(_)) => {
                500
            }
            GenerateError::Study(_) => 400,
        }
    }
}

/// Generate an annotated study from injected stage seams and persist it for
/// `user`. Pure orchestration over the three Epic 9 stages — deterministic for
/// identical seams, so it is unit-testable with fake evaluator / continuation
/// source / LLM provider. The model never sees engine ground truth (#31): the
/// tree it annotates carries only moves, concepts and opening names.
pub async fn generate_study<E, S>(
    eval: &E,
    stats: &S,
    provider: &dyn LlmProvider,
    studies: &StudyService,
    user: &CurrentUser,
    params: &GenerateParams,
    plans: Option<&(dyn MultiAnalyzer + Sync)>,
) -> Result<GenerateOutcome, GenerateError>
where
    E: Evaluator + Sync,
    S: ContinuationSource + Sync,
{
    let mut tree: VariationTree =
        build_tree(eval, stats, &params.start_fen, &params.tree, MODE).await?;

    // Pin plan/threat arrows onto each node before annotation. These are
    // engine/DB-grounded data, not prose, so they never enter the LLM prompt
    // (ADR-0009); `move_tree_from` carries them into the persisted study.
    let shapes = ShapeConfig {
        plan_lines: params.plan_lines,
        threats: params.threats,
    };
    if !shapes.is_off() {
        apply_shapes(plans, &mut tree, &shapes, MODE).await;
    }

    let model = params
        .model
        .clone()
        .unwrap_or_else(|| provider.default_model().to_string());
    let outcome = annotate_tree(provider, &tree, &model, MODE).await?;

    let study = studies
        .create_with_tree(
            user,
            params.database_id,
            params.name.clone(),
            params.global,
            &outcome.tree,
        )
        .await?;

    Ok(GenerateOutcome {
        node_count: outcome.tree.nodes.len(),
        rejected: outcome.rejected,
        study,
    })
}

/// Production wrapper over [`generate_study`]: builds the live engine + DB
/// adapters scoped to `user`, then runs the orchestrator. `engine_limits` bounds
/// each per-position search.
#[allow(clippy::too_many_arguments)]
pub async fn generate_study_live(
    engine: &EngineService,
    reports: &PositionReportService,
    provider: &dyn LlmProvider,
    studies: &StudyService,
    user: &CurrentUser,
    params: &GenerateParams,
    engine_limits: Limits,
) -> Result<GenerateOutcome, GenerateError> {
    let evaluator = EngineEvaluator::new(engine, engine_limits.clone());
    let continuations = ReportContinuations::new(reports, user);

    // A separate depth-bounded multi-PV analyser sources the plan arrows; built
    // only when plan lines are requested (threats need no engine).
    let plan_analyzer = (params.plan_lines > 0).then(|| {
        EnginePlanAnalyzer::new(
            engine,
            engine_limits,
            params.plan_lines.min(MAX_PLAN_LINES) as u16,
        )
    });
    let plans = plan_analyzer
        .as_ref()
        .map(|a| a as &(dyn MultiAnalyzer + Sync));

    generate_study(
        &evaluator,
        &continuations,
        provider,
        studies,
        user,
        params,
        plans,
    )
    .await
}

#[cfg(test)]
#[path = "generate_tests.rs"]
mod tests;
