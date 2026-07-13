//! Study generation pipeline (Epic 9): deterministic preprocessing **stages**
//! that turn engine + DB ground truth into material an LLM later annotates
//! (ADR-0009). The model never drives these stages; it consumes their finished,
//! serializable output.
//!
//! This module wires the concrete adapters that bridge the I/O-free
//! [`tree::build_tree`] walk to the real engine facade and DB layer; the tree
//! types and pruning logic live in [`tree`].

pub mod annotate;
pub mod attack;
pub mod danger;
pub mod danger_generate;
pub mod features;
pub mod generate;
pub mod plan_shapes;
pub mod seed;
pub mod spine;
pub mod tree;

use std::collections::BTreeMap;

use anyhow::Result;
use async_trait::async_trait;

use crate::engine::{Analysis, EngineService, Limits, Score};
use crate::pgn_tree::MoveTree;
use crate::position::CastlingMode;
use crate::search::position::PositionFilter;
use crate::search::report::{MoveReport, PositionReportService};
use crate::server::identity::CurrentUser;

pub use annotate::{
    annotate_tree, build_prompt, build_request, verify_and_commit, AnnotateError,
    AnnotationOutcome, Claim, DraftAnnotation, Rejection,
};
pub use attack::{pawn_storm, AttackConfig, AttackSignal};
pub use danger::{is_only_move, only_move_gap, trap_verdict, DangerConfig, TrapVerdict};
pub use danger_generate::{
    generate_danger_study, generate_danger_study_live, DangerStudyError, DangerStudyOutcome,
    DangerStudyParams, TaggedRole,
};
pub use features::{concepts_of_fen, concepts_of_fen_with, Concepts, KeySquare};
pub use generate::{
    generate_study, generate_study_live, GenerateError, GenerateOutcome, GenerateParams,
};
pub use plan_shapes::{apply_shapes, ShapeConfig, MAX_PLAN_LINES};
pub use seed::{seed_study_from_danger, seed_study_from_tree, SeedOutcome, SeedParams};
pub use spine::{
    walk_danger_spine, DangerKind, DangerNode, DangerRole, DangerTag, DangerTree, MultiAnalyzer,
    Side, SpineConfig, SpineError,
};
pub use tree::{
    build_tree, score_to_cp, select_continuations, Candidate, ContinuationSource, Evaluator,
    TreeConfig, TreeError, VariationNode, VariationTree,
};

/// [`Evaluator`] backed by the pooled engine facade. Each `eval` runs one bounded
/// search and keeps the primary line's score.
pub struct EngineEvaluator<'a> {
    engine: &'a EngineService,
    limits: Limits,
    options: BTreeMap<String, String>,
}

impl<'a> EngineEvaluator<'a> {
    /// Evaluate with the given search `limits` (e.g. a fixed depth). Engine
    /// options default to empty.
    pub fn new(engine: &'a EngineService, limits: Limits) -> Self {
        Self {
            engine,
            limits,
            options: BTreeMap::new(),
        }
    }
}

#[async_trait]
impl Evaluator for EngineEvaluator<'_> {
    async fn eval(&self, fen: &str) -> Result<Option<Score>> {
        let analysis = self
            .engine
            .analyse(fen, &self.limits, &self.options)
            .await?;
        Ok(analysis.score)
    }
}

/// [`ContinuationSource`] backed by the pre-chewed DB layer (issue #28), scoped
/// to one caller.
pub struct ReportContinuations<'a> {
    reports: &'a PositionReportService,
    user: &'a CurrentUser,
    filter: PositionFilter,
}

impl<'a> ReportContinuations<'a> {
    pub fn new(
        reports: &'a PositionReportService,
        user: &'a CurrentUser,
        filter: PositionFilter,
    ) -> Self {
        Self {
            reports,
            user,
            filter,
        }
    }
}

#[async_trait]
impl ContinuationSource for ReportContinuations<'_> {
    async fn continuations(&self, fen: &str) -> Result<Vec<MoveReport>> {
        let report = self
            .reports
            .position_report(self.user, fen, &self.filter)
            .await?;
        Ok(report.moves)
    }
}

/// Build a variation tree from `start_fen` using the live engine and DB layer,
/// scoped to `user`. A thin convenience over [`build_tree`] with the concrete
/// adapters; `engine_limits` bounds each per-position search. `filter` narrows
/// which of `user`'s games feed the continuations (issue #172).
#[allow(clippy::too_many_arguments)]
pub async fn build_variation_tree(
    engine: &EngineService,
    reports: &PositionReportService,
    user: &CurrentUser,
    start_fen: &str,
    config: &TreeConfig,
    filter: &PositionFilter,
    engine_limits: Limits,
    castling: CastlingMode,
) -> Result<VariationTree, TreeError> {
    let evaluator = EngineEvaluator::new(engine, engine_limits);
    let continuations = ReportContinuations::new(reports, user, filter.clone());
    build_tree(&evaluator, &continuations, start_fen, config, castling).await
}

/// [`MultiAnalyzer`] backed by the pooled engine facade. Every opponent position
/// gets one `analyse_multi` search under a fixed **movetime-per-variation**
/// budget (clamped to `MAX_MOVETIME_MS` inside the engine, ADR-0026).
pub struct EngineMultiAnalyzer<'a> {
    engine: &'a EngineService,
    limits: Limits,
    multipv: u16,
}

impl<'a> EngineMultiAnalyzer<'a> {
    /// Search each position for `movetime_ms` milliseconds, returning up to
    /// `multipv` lines. At least two lines are needed for the trap / only-move
    /// gap, so `multipv` is floored at 2.
    pub fn new(engine: &'a EngineService, movetime_ms: u64, multipv: u16) -> Self {
        Self {
            engine,
            limits: Limits {
                movetime_ms: Some(movetime_ms),
                ..Limits::default()
            },
            multipv: multipv.max(2),
        }
    }
}

#[async_trait]
impl MultiAnalyzer for EngineMultiAnalyzer<'_> {
    async fn analyse_multi(&self, fen: &str) -> Result<Vec<Analysis>> {
        self.engine
            .analyse_multi(fen, &self.limits, self.multipv)
            .await
    }
}

/// [`MultiAnalyzer`] for the study **plan-shapes** pass: top-`multipv` PV lines
/// per position under the same **depth** budget the generator already uses for
/// node evals (unlike [`EngineMultiAnalyzer`], which is movetime-based and floors
/// `multipv` at 2 for the danger-map gap). Used to source [`plan_shapes`] arrows.
pub struct EnginePlanAnalyzer<'a> {
    engine: &'a EngineService,
    limits: Limits,
    multipv: u16,
}

impl<'a> EnginePlanAnalyzer<'a> {
    /// Reuse the generator's per-position search `limits`, returning up to
    /// `multipv` principal variations for the plan arrows.
    pub fn new(engine: &'a EngineService, limits: Limits, multipv: u16) -> Self {
        Self {
            engine,
            limits,
            multipv,
        }
    }
}

#[async_trait]
impl MultiAnalyzer for EnginePlanAnalyzer<'_> {
    async fn analyse_multi(&self, fen: &str) -> Result<Vec<Analysis>> {
        self.engine
            .analyse_multi(fen, &self.limits, self.multipv)
            .await
    }
}

/// Walk a repertoire `spine` from `start_fen` against the live engine + DB,
/// scoped to `user`, tagging the dangerous opponent positions (issue #139). A
/// thin convenience over [`walk_danger_spine`] with the concrete adapters;
/// `movetime_ms` is the per-variation search budget and `multipv` the line count.
#[allow(clippy::too_many_arguments)]
pub async fn walk_danger_spine_live(
    engine: &EngineService,
    reports: &PositionReportService,
    user: &CurrentUser,
    spine: &MoveTree,
    start_fen: &str,
    config: &SpineConfig,
    castling: CastlingMode,
    movetime_ms: u64,
    multipv: u16,
) -> Result<DangerTree, SpineError> {
    let analyzer = EngineMultiAnalyzer::new(engine, movetime_ms, multipv);
    // Danger-map generation is out of scope for the #172 filter (per the ADR):
    // the walk always sees every scoped game.
    let continuations = ReportContinuations::new(reports, user, PositionFilter::default());
    walk_danger_spine(
        &analyzer,
        &continuations,
        spine,
        start_fen,
        config,
        castling,
    )
    .await
}
