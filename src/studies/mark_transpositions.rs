//! Standalone transposition-annotation pass for an existing study (issue #174):
//! the transport-agnostic `StudyService::mark_transpositions`. Kept out of
//! `mod.rs` so that already-large file stays under the cap.
//!
//! The Zobrist walk and comment-tagging live in the pure
//! [`crate::pgn_tree::transpositions`] (via [`MoveTree::mark_transpositions`]);
//! this layer only loads and persists the study. The same pass also runs
//! automatically at the end of [`MoveTree::merge_games`](crate::pgn_tree::merge)
//! (`studies/merge.rs`) — this method is for a study built or edited some other
//! way (a plain PGN import, manual grafts, …).

use crate::db::entities::studies;
use crate::pgn_tree::MoveTree;
use crate::server::identity::CurrentUser;

use super::{StudyError, StudyService};

impl StudyService {
    /// Tag transposing lines in a study the caller may write with a note
    /// pointing at the canonical (earlier, mainline-first) node reaching the
    /// same position, and return the refreshed study. Idempotent — re-running
    /// after further edits just refreshes the notes.
    pub async fn mark_transpositions(
        &self,
        user: &CurrentUser,
        study_id: i32,
    ) -> Result<studies::Model, StudyError> {
        let study = self.load_writable(user, study_id).await?;
        let mut tree: MoveTree = serde_json::from_str(&study.tree_json)?;
        tree.mark_transpositions();
        self.persist(study, &tree).await?;
        self.get(user, study_id).await
    }
}

#[cfg(test)]
#[path = "mark_transpositions_tests.rs"]
mod tests;
