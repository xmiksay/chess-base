//! Danger-map study generator (issue #140, ADR-0026 phase 3): the orchestrator
//! that turns a repertoire into a persisted, annotated **danger study**. It ties
//! the phase-2 spine walk (#139) to the same batch annotation + verification pass
//! (#31) and persistence path (#115) the best-line generator uses, so HTTP / MCP
//! stay thin callers.
//!
//! The pipeline (ADR-0026: the engine is the adjudicator, the LLM only annotates):
//!   1. [`walk_danger_spine`] walks the repertoire `spine` into a tagged
//!      [`DangerTree`] — every dangerous opponent move carries a Weapon / Caution
//!      / Off-book role the engine + DB already adjudicated,
//!   2. [`to_variation_tree`] folds that tree into a [`VariationTree`] whose role
//!      tags ride along as concept hints, so the model annotates *danger-aware*,
//!   3. [`annotate_tree`] runs the tool-free LLM pass and verifies every concrete
//!      claim against ground truth before commit,
//!   4. the verified [`MoveTree`] is persisted as a [`studies`] row via
//!      [`StudyService`].
//!
//! The danger tree carries no per-node engine eval (only the role verdict), so a
//! material / quality claim the model invents cannot be ground-truth-confirmed
//! and the verification loop drops it; structural claims (`only_move`, checked
//! against the parent position's legal moves) still verify. Both the dropped
//! claims and the engine-adjudicated role tags are surfaced on the result.
//!
//! [`generate_danger_study`] is generic over the [`MultiAnalyzer`] /
//! [`ContinuationSource`] / [`LlmProvider`] seams so it is unit-testable with
//! injected fakes; [`generate_danger_study_live`] is the production wrapper.

use crate::ai::llm::LlmProvider;
use crate::db::entities::studies;
use crate::engine::EngineService;
use crate::pgn_tree::MoveTree;
use crate::position::{zobrist_of_fen, CastlingMode};
use crate::search::report::PositionReportService;
use crate::server::identity::CurrentUser;
use crate::studies::{StudyError, StudyService};

use super::annotate::{annotate_tree, AnnotateError, Rejection};
use super::features::concepts_of_fen_with;
use super::spine::{
    walk_danger_spine, DangerKind, DangerRole, DangerTag, DangerTree, MultiAnalyzer, SpineConfig,
    SpineError,
};
use super::tree::{eco_for, ContinuationSource, VariationNode, VariationTree};
use super::{EngineMultiAnalyzer, ReportContinuations};

/// Studies are standard chess (mirrors [`crate::studies`]); the danger tree's
/// FENs parse castling rights the normal way.
const MODE: CastlingMode = CastlingMode::Standard;

/// What danger study to generate and where to file it. The walk shape and
/// classifier thresholds live on [`SpineConfig`] (including which side the
/// repertoire plays); the LLM `model` defaults to the provider's own default.
#[derive(Clone, Debug)]
pub struct DangerStudyParams {
    /// Database the new study belongs to.
    pub database_id: i32,
    /// Name for the new study.
    pub name: String,
    /// Make it a global (admin-owned) study; requires admin.
    pub global: bool,
    /// FEN the repertoire walk starts from.
    pub start_fen: String,
    /// The repertoire spine — the user's intended move tree to walk.
    pub spine: MoveTree,
    /// Walk depth/breadth + classifier thresholds (and `our_side`).
    pub spine_config: SpineConfig,
    /// LLM model id; `None` ⇒ the provider's default model.
    pub model: Option<String>,
}

/// One danger role the walk tagged, surfaced on the result so the caller knows
/// which committed moves are weapons / cautions / off-book without re-walking.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TaggedRole {
    /// Node id in the persisted tree (ids are preserved through annotation).
    pub node_id: usize,
    /// SAN of the tagged move (a tagged node always reached the position by a
    /// move; the move-less root is never tagged).
    pub san: Option<String>,
    pub kind: DangerKind,
    pub role: DangerRole,
}

/// The persisted danger study plus what the pipeline surfaced: the dropped claims
/// and the engine-adjudicated role tags.
#[derive(Clone, Debug)]
pub struct DangerStudyOutcome {
    /// The newly created, annotated study row.
    pub study: studies::Model,
    /// Number of nodes in the committed move tree.
    pub node_count: usize,
    /// Claims / glyphs the ground-truth verification rejected (never committed).
    pub rejected: Vec<Rejection>,
    /// The danger role tags carried into the study (already engine-adjudicated in
    /// phase 2), most dangerous lines first by walk order.
    pub roles: Vec<TaggedRole>,
}

/// Why generating a danger study failed. Transport-agnostic; the HTTP / MCP layer
/// maps [`client_message`](Self::client_message) / [`http_status_hint`](Self::http_status_hint)
/// onto its own envelope so engine / DB / LLM internals never leak to clients.
#[derive(Debug, thiserror::Error)]
pub enum DangerStudyError {
    /// The spine walk rejected the start position or its engine / DB lookups failed.
    #[error(transparent)]
    Spine(#[from] SpineError),
    /// The annotation pass failed before it could verify and commit anything.
    #[error(transparent)]
    Annotate(#[from] AnnotateError),
    /// Persisting the generated study failed (ownership or database error).
    #[error(transparent)]
    Study(#[from] StudyError),
}

impl DangerStudyError {
    /// A safe, client-facing message: it names the *stage* that failed without
    /// surfacing a raw `DbErr`, engine output or provider transport detail.
    pub fn client_message(&self) -> String {
        match self {
            DangerStudyError::Spine(SpineError::InvalidFen(msg)) => format!("invalid FEN: {msg}"),
            DangerStudyError::Spine(SpineError::Source(_)) => {
                "could not read the position from the database".into()
            }
            DangerStudyError::Annotate(AnnotateError::Provider(_)) => {
                "the language model request failed".into()
            }
            DangerStudyError::Annotate(AnnotateError::EmptyResponse)
            | DangerStudyError::Annotate(AnnotateError::Parse(_)) => {
                "the language model returned an unusable response".into()
            }
            DangerStudyError::Study(StudyError::Forbidden) => StudyError::Forbidden.to_string(),
            DangerStudyError::Study(StudyError::Db(_)) => "study could not be saved".into(),
            DangerStudyError::Study(other) => other.to_string(),
        }
    }

    /// HTTP status hint (raw code) so the route maps failures without this module
    /// depending on `axum`: client mistakes are 4xx, upstream/internal faults 5xx.
    pub fn http_status_hint(&self) -> u16 {
        match self {
            DangerStudyError::Spine(SpineError::InvalidFen(_)) => 400,
            DangerStudyError::Spine(SpineError::Source(_)) => 500,
            DangerStudyError::Annotate(_) => 502,
            DangerStudyError::Study(StudyError::Forbidden) => 403,
            DangerStudyError::Study(StudyError::NotFound) => 404,
            DangerStudyError::Study(StudyError::Db(_))
            | DangerStudyError::Study(StudyError::Tree(_)) => 500,
            DangerStudyError::Study(_) => 400,
        }
    }
}

/// Generate an annotated danger study from injected seams and persist it for
/// `user`. Pure orchestration over the four stages — deterministic for identical
/// seams, so it is unit-testable with fake analyzer / continuation source / LLM
/// provider. The model never sees engine ground truth (#31): the tree it
/// annotates carries only moves, concepts, opening names and the role hints.
pub async fn generate_danger_study<A, S>(
    analyzer: &A,
    stats: &S,
    provider: &dyn LlmProvider,
    studies: &StudyService,
    user: &CurrentUser,
    params: &DangerStudyParams,
) -> Result<DangerStudyOutcome, DangerStudyError>
where
    A: MultiAnalyzer + Sync,
    S: ContinuationSource + Sync,
{
    let danger = walk_danger_spine(
        analyzer,
        stats,
        &params.spine,
        &params.start_fen,
        &params.spine_config,
        MODE,
    )
    .await?;

    let roles = collect_roles(&danger);
    let tree = to_variation_tree(&danger);

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

    Ok(DangerStudyOutcome {
        node_count: outcome.tree.nodes.len(),
        rejected: outcome.rejected,
        roles,
        study,
    })
}

/// Production wrapper over [`generate_danger_study`]: builds the live multi-PV
/// engine analyzer + DB continuation source scoped to `user`, then runs the
/// orchestrator. `movetime_ms` is the per-variation search budget and `multipv`
/// the line count (floored at 2 for the trap / only-move gap).
#[allow(clippy::too_many_arguments)]
pub async fn generate_danger_study_live(
    engine: &EngineService,
    reports: &PositionReportService,
    provider: &dyn LlmProvider,
    studies: &StudyService,
    user: &CurrentUser,
    params: &DangerStudyParams,
    movetime_ms: u64,
    multipv: u16,
) -> Result<DangerStudyOutcome, DangerStudyError> {
    let analyzer = EngineMultiAnalyzer::new(engine, movetime_ms, multipv);
    let continuations = ReportContinuations::new(reports, user);
    generate_danger_study(&analyzer, &continuations, provider, studies, user, params).await
}

/// The danger role tags, in walk (breadth-first) order — shallow, on-book lines
/// first. The move-less root never carries a tag, so every entry has a SAN.
fn collect_roles(danger: &DangerTree) -> Vec<TaggedRole> {
    danger
        .nodes
        .iter()
        .filter_map(|n| {
            n.tag.as_ref().map(|tag| TaggedRole {
                node_id: n.id,
                san: n.san.clone(),
                kind: tag.kind,
                role: tag.role,
            })
        })
        .collect()
}

/// Fold a tagged danger tree into a [`VariationTree`] the annotation pass can
/// consume. The shape (ids, parents, SANs, children) is carried 1:1; per node it
/// recomputes the Zobrist key, ECO name and strategic concepts, and appends each
/// tagged node's role as a synthetic concept tag so the model annotates
/// danger-aware. `eval` is left `None` — the danger tree stores only the role
/// verdict, so quality claims cannot be confirmed and the verifier drops them.
///
/// Shared with the LLM-free seed path (issue #155): seeding a study from a danger
/// map folds the tagged tree the same way, then persists it without annotation.
pub fn to_variation_tree(danger: &DangerTree) -> VariationTree {
    let nodes = danger
        .nodes
        .iter()
        .map(|n| {
            let zob = zobrist_of_fen(&n.fen, MODE).ok();
            let mut concepts = concepts_of_fen_with(&n.fen, MODE).unwrap_or_default();
            if let Some(tag) = &n.tag {
                concepts.tags.push(role_concept(tag));
            }
            VariationNode {
                id: n.id,
                parent: n.parent,
                san: n.san.clone(),
                fen: n.fen.clone(),
                zobrist: zob.map(|z| format!("{z:016x}")).unwrap_or_default(),
                ply: n.ply,
                eval: None,
                stats: None,
                eco: zob.and_then(eco_for),
                concepts,
                children: n.children.clone(),
            }
        })
        .collect();
    VariationTree {
        nodes,
        root: danger.root,
    }
}

/// A human-readable concept tag describing a node's danger role, fed to the model
/// so its annotation reflects *why* the move is dangerous (no engine numbers — the
/// verdict is already adjudicated; the model only writes prose around it).
fn role_concept(tag: &DangerTag) -> String {
    match (tag.kind, tag.role) {
        (DangerKind::Trap, DangerRole::Weapon) => {
            "danger weapon: a trap whose downside stays bounded when the opponent finds the best reply".into()
        }
        (DangerKind::Trap, DangerRole::Caution) => {
            "danger caution: a baiting move the best reply refutes — warn, do not recommend it".into()
        }
        (DangerKind::OnlyMove, DangerRole::Weapon) => {
            "danger weapon: a narrow only-move path the opponent frequently misses".into()
        }
        (DangerKind::Attack, _) => {
            "danger caution: this move concedes a pawn storm marching toward your king — warn about the attack".into()
        }
        (DangerKind::OffBook, _) => {
            "danger off-book: a reply order the repertoire does not yet answer".into()
        }
        _ => "danger: a move the engine and DB flagged as practically tricky".into(),
    }
}

#[cfg(test)]
#[path = "danger_generate_tests.rs"]
mod tests;
