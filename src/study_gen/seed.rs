//! Study **seeding** (issue #155): persist a preprocessing tree straight into a
//! study, server-side, with no LLM and no client round-trip.
//!
//! The data tools (`opening_tree` / `danger_map`, ADR-0027) already build the
//! tree the study generators consume, and [`StudyService`] already knows how to
//! persist one. Without this, an MCP client has to ship the whole tree back out
//! (120 nodes overflow the tool-result budget), hand-serialize it into PGN
//! (error-prone variation placement), and re-import it — the server building a
//! tree, shipping it out, and rebuilding an equivalent one.
//!
//! Seeding is exactly what [`generate_study`](super::generate::generate_study)
//! does **minus** the LLM annotation step, so it stays LLM-free inside the tool
//! (ADR-0027: the model that adds prose is the MCP client, via `study_annotate`).
//! The conversion ([`move_tree_from`]) carries the set-up `start_fen` from the
//! root, so a study seeded from a non-startpos FEN records it (ADR-0028).

use crate::db::entities::studies;
use crate::server::identity::CurrentUser;
use crate::studies::{StudyError, StudyService};

use super::annotate::move_tree_from;
use super::danger_generate::to_variation_tree;
use super::spine::DangerTree;
use super::tree::VariationTree;

/// Where to file a seeded study: the same ownership knobs as
/// [`StudyService::create`](crate::studies::StudyService::create). `global` makes
/// it an admin-owned study visible to everyone and requires admin.
#[derive(Clone, Debug)]
pub struct SeedParams {
    /// Database the new study belongs to.
    pub database_id: i32,
    /// Name for the new study.
    pub name: String,
    /// Make it a global (admin-owned) study; requires admin.
    pub global: bool,
}

/// The persisted study plus its committed node count — the whole tool response
/// (no tree JSON, which is the point: an id, not 100k chars).
#[derive(Clone, Debug)]
pub struct SeedOutcome {
    /// The newly created study row.
    pub study: studies::Model,
    /// Number of nodes in the persisted move tree.
    pub node_count: usize,
}

/// Persist a built [`VariationTree`] as a new study owned by `user`. The shared
/// tail of the seed paths: convert to a [`MoveTree`](crate::pgn_tree::MoveTree)
/// (every move already `apply_san`-validated during the build, so the tree is
/// correct by construction) and hand it to the same `create_with_tree` path the
/// generators use, so ownership / `global` gating stays in one place.
pub async fn seed_study_from_tree(
    studies: &StudyService,
    user: &CurrentUser,
    tree: &VariationTree,
    params: &SeedParams,
) -> Result<SeedOutcome, StudyError> {
    let move_tree = move_tree_from(tree);
    let node_count = move_tree.nodes.len();
    let study = studies
        .create_with_tree(
            user,
            params.database_id,
            params.name.clone(),
            params.global,
            &move_tree,
        )
        .await?;
    Ok(SeedOutcome { study, node_count })
}

/// Persist a built [`DangerTree`] as a new study owned by `user`. Folds the tagged
/// danger tree into a [`VariationTree`] (role tags ride along as concept hints,
/// the same fold the danger-map generator uses) and seeds from that.
pub async fn seed_study_from_danger(
    studies: &StudyService,
    user: &CurrentUser,
    danger: &DangerTree,
    params: &SeedParams,
) -> Result<SeedOutcome, StudyError> {
    let tree = to_variation_tree(danger);
    seed_study_from_tree(studies, user, &tree, params).await
}

#[cfg(test)]
#[path = "seed_tests.rs"]
mod tests;
