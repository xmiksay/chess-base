//! Transport-agnostic study service: the one place study CRUD and the
//! [`MoveTree`] edits live, so the HTTP routes and the planned MCP `/mcp` tools
//! are thin callers (the `repo::*`-behind-`routes/mcp.rs` split from the `site`
//! project).
//!
//! It carries no HTTP/MCP concerns: every method takes a [`CurrentUser`] and
//! returns plain models or a [`StudyError`] the transport maps to a response.
//! Ownership follows ADR 0007 / 0011 — reads see the caller's studies plus global
//! (`owner_id IS NULL`) ones; writes touch only the caller's own studies, and a
//! global study requires admin.

use sea_orm::{
    ActiveModelTrait, DatabaseConnection, DbErr, EntityTrait, QueryFilter, QueryOrder, Set,
};

use crate::db::entities::studies;
use crate::pgn_tree::MoveTree;
use crate::position::{legal_sans, replay, CastlingMode, PositionError, STARTPOS_FEN};
use crate::server::identity::{assert_admin, scope, CurrentUser};

/// Studies are standard chess; castling rights parse the normal way.
const MODE: CastlingMode = CastlingMode::Standard;

/// Why a study operation failed. Transport-agnostic — the HTTP / MCP layer maps
/// each variant onto its own status / error envelope.
#[derive(Debug, thiserror::Error)]
pub enum StudyError {
    /// No study with that id is visible to the caller.
    #[error("study not found")]
    NotFound,
    /// Authenticated but not permitted: another user's study, or a global study
    /// touched by a non-admin.
    #[error("not permitted")]
    Forbidden,
    /// `node_id` is not a node in the study's tree.
    #[error("node {0} not found in study")]
    InvalidNode(usize),
    /// `san` is not a legal move in the position at the target node.
    #[error("illegal move '{san}' in position {fen}")]
    IllegalMove { san: String, fen: String },
    /// The stored `tree_json` could not be (de)serialized — a corrupt tree.
    #[error("corrupt study tree: {0}")]
    Tree(#[from] serde_json::Error),
    /// Replaying the tree to the target node hit an illegal position/move.
    #[error(transparent)]
    Position(#[from] PositionError),
    /// Underlying database error (never surfaced verbatim to clients).
    #[error(transparent)]
    Db(#[from] DbErr),
}

/// Study CRUD + move-tree edits over the `studies` table. Holds a connection
/// handle (cheap to clone — SeaORM wraps an `Arc`'d pool).
#[derive(Clone)]
pub struct StudyService {
    db: DatabaseConnection,
}

impl StudyService {
    pub fn new(db: DatabaseConnection) -> Self {
        Self { db }
    }

    /// Create an empty study (just the move-tree root). `global` makes it an
    /// admin-owned study visible to everyone (`owner_id IS NULL`) and requires
    /// admin; otherwise it belongs to the caller.
    pub async fn create(
        &self,
        user: &CurrentUser,
        database_id: i32,
        name: impl Into<String>,
        global: bool,
    ) -> Result<studies::Model, StudyError> {
        let owner_id = if global {
            assert_admin(user).map_err(|_| StudyError::Forbidden)?;
            None
        } else {
            Some(user.id.clone())
        };
        let tree_json = serde_json::to_string(&MoveTree::new())?;
        let model = studies::ActiveModel {
            database_id: Set(database_id),
            owner_id: Set(owner_id),
            name: Set(name.into()),
            tree_json: Set(tree_json),
            ..Default::default()
        }
        .insert(&self.db)
        .await?;
        Ok(model)
    }

    /// All studies visible to the caller (own ∪ global), oldest first.
    pub async fn list(&self, user: &CurrentUser) -> Result<Vec<studies::Model>, StudyError> {
        let rows = studies::Entity::find()
            .filter(scope(studies::Column::OwnerId, user))
            .order_by_asc(studies::Column::Id)
            .all(&self.db)
            .await?;
        Ok(rows)
    }

    /// A single study, if it is visible to the caller.
    pub async fn get(&self, user: &CurrentUser, id: i32) -> Result<studies::Model, StudyError> {
        studies::Entity::find_by_id(id)
            .filter(scope(studies::Column::OwnerId, user))
            .one(&self.db)
            .await?
            .ok_or(StudyError::NotFound)
    }

    /// Delete a study the caller may write.
    pub async fn delete(&self, user: &CurrentUser, id: i32) -> Result<(), StudyError> {
        let study = self.load_writable(user, id).await?;
        studies::Entity::delete_by_id(study.id)
            .exec(&self.db)
            .await?;
        Ok(())
    }

    /// Append `san` as a child of `from_node_id`, after validating it is legal in
    /// the position reached by replaying that node's line. Returns the new node id.
    pub async fn add_move(
        &self,
        user: &CurrentUser,
        study_id: i32,
        from_node_id: usize,
        san: &str,
    ) -> Result<usize, StudyError> {
        let study = self.load_writable(user, study_id).await?;
        let mut tree: MoveTree = serde_json::from_str(&study.tree_json)?;

        let line = tree
            .line_to(from_node_id)
            .ok_or(StudyError::InvalidNode(from_node_id))?;
        let fen = fen_at(&line)?;
        // Validate against the legal-move set at this position; accept user input
        // with or without a check/mate suffix and store the canonical SAN.
        let canonical = legal_sans(&fen, MODE)?
            .into_iter()
            .find(|legal| san_core(legal) == san_core(san))
            .ok_or_else(|| StudyError::IllegalMove {
                san: san.to_string(),
                fen,
            })?;

        let new_id = tree.add_move(from_node_id, canonical);
        self.persist(study, &tree).await?;
        Ok(new_id)
    }

    /// Attach a comment and/or a NAG to a node of a study the caller may write.
    pub async fn annotate(
        &self,
        user: &CurrentUser,
        study_id: i32,
        node_id: usize,
        comment: Option<String>,
        nag: Option<u8>,
    ) -> Result<(), StudyError> {
        let study = self.load_writable(user, study_id).await?;
        let mut tree: MoveTree = serde_json::from_str(&study.tree_json)?;
        if tree.line_to(node_id).is_none() {
            return Err(StudyError::InvalidNode(node_id));
        }
        if let Some(comment) = comment {
            tree.set_comment(node_id, comment);
        }
        if let Some(nag) = nag {
            tree.add_nag(node_id, nag);
        }
        self.persist(study, &tree).await?;
        Ok(())
    }

    /// Load a study by id and enforce the write guard: the caller must own it, or
    /// be admin for a global one. `NotFound` hides ids that don't exist at all.
    async fn load_writable(
        &self,
        user: &CurrentUser,
        id: i32,
    ) -> Result<studies::Model, StudyError> {
        let study = studies::Entity::find_by_id(id)
            .one(&self.db)
            .await?
            .ok_or(StudyError::NotFound)?;
        assert_can_write(&study, user)?;
        Ok(study)
    }

    /// Persist an edited tree back onto an existing study row.
    async fn persist(&self, study: studies::Model, tree: &MoveTree) -> Result<(), StudyError> {
        let mut active: studies::ActiveModel = study.into();
        active.tree_json = Set(serde_json::to_string(tree)?);
        active.update(&self.db).await?;
        Ok(())
    }
}

/// Write guard (ADR 0007 / 0011): a study is writable only by its owner; a global
/// study (`owner_id IS NULL`) requires admin.
fn assert_can_write(study: &studies::Model, user: &CurrentUser) -> Result<(), StudyError> {
    match &study.owner_id {
        None => assert_admin(user).map_err(|_| StudyError::Forbidden),
        Some(owner) if *owner == user.id => Ok(()),
        Some(_) => Err(StudyError::Forbidden),
    }
}

/// FEN of the position reached by replaying a node's SAN line from the start.
fn fen_at(line: &[String]) -> Result<String, PositionError> {
    match replay(STARTPOS_FEN, line, MODE)?.last() {
        Some(ply) => Ok(ply.fen.clone()),
        None => Ok(STARTPOS_FEN.to_string()),
    }
}

/// SAN without its trailing check/mate marker, so `Qh5+` matches the generated
/// `Qh5`.
fn san_core(san: &str) -> &str {
    san.trim_end_matches(['+', '#'])
}

#[cfg(test)]
mod tests;
