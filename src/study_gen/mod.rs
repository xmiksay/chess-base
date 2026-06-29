//! Study generation pipeline (Epic 9): deterministic preprocessing **stages**
//! that turn engine + DB ground truth into material an LLM later annotates
//! (ADR-0009). The model never drives these stages; it consumes their finished,
//! serializable output.
//!
//! This module wires the concrete adapters that bridge the I/O-free
//! [`tree::build_tree`] walk to the real engine facade and DB layer; the tree
//! types and pruning logic live in [`tree`].

pub mod annotate;
pub mod features;
pub mod tree;

use std::collections::BTreeMap;

use anyhow::Result;
use async_trait::async_trait;

use crate::engine::{EngineService, Limits, Score};
use crate::position::CastlingMode;
use crate::search::report::{MoveReport, PositionReportService};
use crate::server::identity::CurrentUser;

pub use annotate::{
    annotate_tree, build_prompt, build_request, verify_and_commit, AnnotateError,
    AnnotationOutcome, Claim, DraftAnnotation, Rejection,
};
pub use features::{concepts_of_fen, concepts_of_fen_with, Concepts, KeySquare};
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
}

impl<'a> ReportContinuations<'a> {
    pub fn new(reports: &'a PositionReportService, user: &'a CurrentUser) -> Self {
        Self { reports, user }
    }
}

#[async_trait]
impl ContinuationSource for ReportContinuations<'_> {
    async fn continuations(&self, fen: &str) -> Result<Vec<MoveReport>> {
        let report = self.reports.position_report(self.user, fen).await?;
        Ok(report.moves)
    }
}

/// Build a variation tree from `start_fen` using the live engine and DB layer,
/// scoped to `user`. A thin convenience over [`build_tree`] with the concrete
/// adapters; `engine_limits` bounds each per-position search.
pub async fn build_variation_tree(
    engine: &EngineService,
    reports: &PositionReportService,
    user: &CurrentUser,
    start_fen: &str,
    config: &TreeConfig,
    engine_limits: Limits,
    castling: CastlingMode,
) -> Result<VariationTree, TreeError> {
    let evaluator = EngineEvaluator::new(engine, engine_limits);
    let continuations = ReportContinuations::new(reports, user);
    build_tree(&evaluator, &continuations, start_fen, config, castling).await
}
