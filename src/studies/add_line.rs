//! Graft a single SAN line into a study from the standard start (issue #173):
//! the position-explorer "Add line to study" action. Builds a linear source
//! [`MoveTree`] from the line and reuses [`MoveTree::graft_subtree`]'s
//! legality-checked, deduped merge (ADR-0032) — so re-adding the same line is a
//! no-op and a line sharing a prefix with an existing one only appends the tail.

use sea_orm::{ActiveModelTrait, Set};

use crate::db::entities::studies;
use crate::games::export::linear_tree;
use crate::pgn_tree::MoveTree;
use crate::position::STARTPOS_FEN;
use crate::server::identity::CurrentUser;

use super::{StudyError, StudyService};

impl StudyService {
    /// Graft `sans` (from the standard start) into a study as deduped,
    /// legality-checked variations. `study_id` set ⇒ graft into that existing
    /// study (must be writable, standard start); `None` ⇒ create a new study
    /// (`name` and `database_id` required) filed into `folder_id`. `comment`,
    /// when given, is attached to the line's final node — e.g. the position
    /// explorer's "N games, W/D/L" stat for that position.
    #[allow(clippy::too_many_arguments)]
    pub async fn add_line(
        &self,
        user: &CurrentUser,
        sans: &[String],
        study_id: Option<i32>,
        database_id: Option<i32>,
        name: Option<String>,
        folder_id: Option<i32>,
        comment: Option<String>,
    ) -> Result<studies::Model, StudyError> {
        if sans.is_empty() {
            return Err(StudyError::InvalidEdit("no moves to add".into()));
        }
        let src = linear_tree(sans);

        let final_id = match study_id {
            Some(id) => {
                let study = self.load_writable(user, id).await?;
                let mut tree: MoveTree = serde_json::from_str(&study.tree_json)?;
                if tree.start_position() != STARTPOS_FEN {
                    return Err(StudyError::InvalidEdit(
                        "cannot add a line to a study with a set-up start position".into(),
                    ));
                }
                graft_line(&mut tree, &src, sans, comment)?;
                self.persist(study, &tree).await?;
                id
            }
            None => {
                let name = name
                    .map(|n| n.trim().to_string())
                    .filter(|n| !n.is_empty())
                    .ok_or_else(|| {
                        StudyError::InvalidEdit("a name is required for a new study".into())
                    })?;
                let database_id = database_id.ok_or_else(|| {
                    StudyError::InvalidEdit("a database is required for a new study".into())
                })?;
                if let Some(folder_id) = folder_id {
                    self.assert_folder_writable(user, folder_id).await?;
                }
                let mut tree = MoveTree::new();
                graft_line(&mut tree, &src, sans, comment)?;
                let model = studies::ActiveModel {
                    database_id: Set(database_id),
                    owner_id: Set(Some(user.id.clone())),
                    name: Set(name),
                    tree_json: Set(serde_json::to_string(&tree)?),
                    folder_id: Set(folder_id),
                    ..Default::default()
                }
                .insert(&self.db)
                .await?;
                model.id
            }
        };
        self.get(user, final_id).await
    }
}

/// Graft `src` (a linear line built from `sans`) into `tree` at its root, then
/// attach `comment` to the line's final node. Errors if the line could not be
/// fully grafted — an illegal move anywhere in `sans`, since [`graft_subtree`]
/// silently drops an illegal move and its remainder rather than failing.
///
/// [`graft_subtree`]: MoveTree::graft_subtree
fn graft_line(
    tree: &mut MoveTree,
    src: &MoveTree,
    sans: &[String],
    comment: Option<String>,
) -> Result<(), StudyError> {
    tree.graft_subtree(tree.root, src);
    let leaf = tree
        .resolve_line(tree.root, sans)
        .ok_or_else(|| StudyError::InvalidEdit("line contains an illegal move".into()))?;
    if let Some(comment) = comment {
        tree.set_comment(leaf, comment);
    }
    Ok(())
}

#[cfg(test)]
#[path = "add_line_tests.rs"]
mod tests;
