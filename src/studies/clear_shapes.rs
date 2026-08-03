//! Bulk shape-clear pass for an existing study (issue #191): remove every
//! generated plan/threat arrow, or every shape regardless of origin, across
//! the whole tree in one call — the counterpart to clearing shapes node-by-
//! node via [`StudyService::set_shapes`](super::StudyService::set_shapes).
//!
//! Kept out of `mod.rs` (already over the file-size cap), mirroring
//! `mark_transpositions.rs`.

use serde::Deserialize;

use crate::db::entities::studies;
use crate::pgn_tree::MoveTree;
use crate::server::identity::CurrentUser;
use crate::study_gen::plan_shapes::merge_shapes;

use super::{StudyError, StudyService};

/// What [`StudyService::clear_shapes`] removes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClearShapesScope {
    /// Only the plan/threat brushes a generate/analyse pass pins
    /// (`plan1..plan3`, `plan1d..plan3d`, `threat`, `study_gen::plan_shapes`'s
    /// `generated_brush`) — user-drawn shapes are left alone.
    Generated,
    /// Every shape on every node, generated or user-drawn.
    All,
}

impl StudyService {
    /// Clear shapes across every node of a study the caller may write, per
    /// `scope`, and return the refreshed study.
    pub async fn clear_shapes(
        &self,
        user: &CurrentUser,
        id: i32,
        scope: ClearShapesScope,
    ) -> Result<studies::Model, StudyError> {
        let study = self.load_writable(user, id).await?;
        let mut tree: MoveTree = serde_json::from_str(&study.tree_json)?;

        for node in &mut tree.nodes {
            node.shapes = match scope {
                ClearShapesScope::All => Vec::new(),
                ClearShapesScope::Generated => merge_shapes(&node.shapes, Vec::new()),
            };
        }

        self.persist(study, &tree).await?;
        self.get(user, id).await
    }
}

#[cfg(test)]
#[path = "clear_shapes_tests.rs"]
mod tests;
