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

pub mod routes;

use crate::db::entities::studies;
use crate::pgn_tree::pgn::{self, PgnError};
use crate::pgn_tree::{lichess, MoveTree, Shape, TreeError};
use crate::position::{
    apply_san, legal_sans, replay, uci_to_san, CastlingMode, PositionError, STARTPOS_FEN,
};
use crate::server::identity::{assert_admin, assert_can_write, scope, CurrentUser};

/// Studies are standard chess; castling rights parse the normal way.
const MODE: CastlingMode = CastlingMode::Standard;

/// A move to append to a study node, given either as SAN or as UCI long
/// algebraic (`g1f3`). UCI sidesteps SAN's strict disambiguation — e.g. a
/// redundant `Ref1` where `Rf1` is the only legal rook move — so an agent can
/// always pass the engine's own UCI output without reformatting (issue #125).
pub enum MoveInput {
    San(String),
    Uci(String),
}

/// The result of appending a move: the new node id, the FEN of the position it
/// reaches, and the canonical SAN that was stored. Returned so a caller chaining
/// moves over MCP need not re-derive the position itself (issue #125).
#[derive(Debug, Clone, serde::Serialize)]
pub struct AddedMove {
    pub node_id: usize,
    pub fen: String,
    pub san: String,
}

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
    /// A structural edit was rejected (e.g. reordering or deleting the root).
    #[error("{0}")]
    InvalidEdit(String),
    /// `san` is not a legal move in the position at the target node.
    #[error("illegal move '{san}' in position {fen}")]
    IllegalMove { san: String, fen: String },
    /// The stored `tree_json` could not be (de)serialized — a corrupt tree.
    #[error("corrupt study tree: {0}")]
    Tree(#[from] serde_json::Error),
    /// PGN import/export failed (malformed input or an illegal move in it).
    #[error(transparent)]
    Pgn(#[from] PgnError),
    /// Replaying the tree to the target node hit an illegal position/move.
    #[error(transparent)]
    Position(#[from] PositionError),
    /// Underlying database error (never surfaced verbatim to clients).
    #[error(transparent)]
    Db(#[from] DbErr),
}

/// Map a pure tree edit failure onto the transport-agnostic study error: a
/// missing node is a `404`-style `InvalidNode`, a rootless edit a `400`-style
/// `InvalidEdit`.
impl From<TreeError> for StudyError {
    fn from(err: TreeError) -> Self {
        match err {
            TreeError::NoSuchNode(id) => StudyError::InvalidNode(id),
            TreeError::NoParent(_) => StudyError::InvalidEdit(err.to_string()),
        }
    }
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
        self.insert(user, database_id, name, global, &MoveTree::new())
            .await
    }

    /// Import a PGN game as a new study. The first game's mainline, variations,
    /// comments and NAGs are parsed into a [`MoveTree`] (every SAN validated for
    /// legality); ownership/`global` follow [`create`](Self::create).
    pub async fn import_pgn(
        &self,
        user: &CurrentUser,
        database_id: i32,
        name: impl Into<String>,
        pgn: &str,
        global: bool,
    ) -> Result<studies::Model, StudyError> {
        let tree = pgn::from_pgn(pgn)?;
        self.insert(user, database_id, name, global, &tree).await
    }

    /// Persist a pre-built [`MoveTree`] as a new study. The study-generation
    /// orchestrator (#115) hands the verified, annotated tree straight to the same
    /// insert path [`create`](Self::create) / [`import_pgn`](Self::import_pgn) use,
    /// so ownership/`global` gating stays in one place.
    pub async fn create_with_tree(
        &self,
        user: &CurrentUser,
        database_id: i32,
        name: impl Into<String>,
        global: bool,
        tree: &MoveTree,
    ) -> Result<studies::Model, StudyError> {
        self.insert(user, database_id, name, global, tree).await
    }

    /// Export a study the caller may read as standard PGN movetext (no headers).
    /// NAGs, comments and pinned board shapes (`[%csl]`/`[%cal]`) are always
    /// preserved; `include_eval` keeps the per-move `[%eval]` annotations (the
    /// extended export, issue #120) or strips them for a plain export.
    pub async fn export_pgn(
        &self,
        user: &CurrentUser,
        id: i32,
        include_eval: bool,
    ) -> Result<String, StudyError> {
        let mut tree = self.load_tree(user, id).await?;
        if !include_eval {
            for node in &mut tree.nodes {
                node.eval = None;
            }
        }
        Ok(pgn::to_pgn(&tree)?)
    }

    /// Export a study the caller may read as a Lichess-study chapter: PGN header
    /// tags (`Event` = study name) plus the same annotated movetext, ready to
    /// import into lichess.org/study or version in git (issue #32).
    pub async fn export_lichess(&self, user: &CurrentUser, id: i32) -> Result<String, StudyError> {
        let study = self.get(user, id).await?;
        let tree: MoveTree = serde_json::from_str(&study.tree_json)?;
        Ok(lichess::to_lichess_study(&study.name, &tree)?)
    }

    /// Load and deserialize the move tree of a study visible to the caller.
    async fn load_tree(&self, user: &CurrentUser, id: i32) -> Result<MoveTree, StudyError> {
        let study = self.get(user, id).await?;
        Ok(serde_json::from_str(&study.tree_json)?)
    }

    /// Insert a new study row with the given tree, resolving ownership: `global`
    /// stores `owner_id NULL` (requires admin), otherwise the caller owns it.
    /// Shared body of [`create`](Self::create) and [`import_pgn`](Self::import_pgn).
    async fn insert(
        &self,
        user: &CurrentUser,
        database_id: i32,
        name: impl Into<String>,
        global: bool,
        tree: &MoveTree,
    ) -> Result<studies::Model, StudyError> {
        let owner_id = if global {
            assert_admin(user).map_err(|_| StudyError::Forbidden)?;
            None
        } else {
            Some(user.id.clone())
        };
        let tree_json = serde_json::to_string(tree)?;
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

    /// Rename a study the caller may write. Returns the updated row.
    pub async fn rename(
        &self,
        user: &CurrentUser,
        id: i32,
        name: impl Into<String>,
    ) -> Result<studies::Model, StudyError> {
        let study = self.load_writable(user, id).await?;
        let mut active: studies::ActiveModel = study.into();
        active.name = Set(name.into());
        let model = active.update(&self.db).await?;
        Ok(model)
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
    /// Thin wrapper over [`add_move_detailed`](Self::add_move_detailed) for callers
    /// (the HTTP route) that only need the id.
    pub async fn add_move(
        &self,
        user: &CurrentUser,
        study_id: i32,
        from_node_id: usize,
        san: &str,
    ) -> Result<usize, StudyError> {
        self.add_move_detailed(
            user,
            study_id,
            from_node_id,
            MoveInput::San(san.to_string()),
        )
        .await
        .map(|added| added.node_id)
    }

    /// Append a move (SAN or UCI) as a child of `from_node_id` and report back the
    /// new node id, the FEN it reaches and the canonical SAN stored. The move is
    /// validated against the legal moves in the position at that node.
    pub async fn add_move_detailed(
        &self,
        user: &CurrentUser,
        study_id: i32,
        from_node_id: usize,
        mv: MoveInput,
    ) -> Result<AddedMove, StudyError> {
        let study = self.load_writable(user, study_id).await?;
        let mut tree: MoveTree = serde_json::from_str(&study.tree_json)?;

        let line = tree
            .line_to(from_node_id)
            .ok_or(StudyError::InvalidNode(from_node_id))?;
        let fen = fen_at(&line)?;
        let canonical = resolve_move(&fen, &mv)?;
        // The position after the move — handed back so an agent chaining moves
        // doesn't replay the line itself. `canonical` is already legal here.
        let (after_fen, _) = apply_san(&fen, &canonical, MODE)?;

        let new_id = tree.add_move(from_node_id, canonical.clone());
        self.persist(study, &tree).await?;
        Ok(AddedMove {
            node_id: new_id,
            fen: after_fen,
            san: canonical,
        })
    }

    /// Set the comment and/or toggle a NAG on a node of a study the caller may
    /// write. Re-sending the same NAG removes it; a move-quality glyph replaces
    /// any other ($1–$6 are mutually exclusive). See [`MoveTree::toggle_nag`].
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
            tree.toggle_nag(node_id, nag);
        }
        self.persist(study, &tree).await?;
        Ok(())
    }

    /// Pin a plan to a node: replace its board shapes (arrows / highlights) on a
    /// study the caller may write. An empty `shapes` clears the pin. Same
    /// ownership guard as [`annotate`](Self::annotate).
    pub async fn set_shapes(
        &self,
        user: &CurrentUser,
        study_id: i32,
        node_id: usize,
        shapes: Vec<Shape>,
    ) -> Result<(), StudyError> {
        let study = self.load_writable(user, study_id).await?;
        let mut tree: MoveTree = serde_json::from_str(&study.tree_json)?;
        if tree.line_to(node_id).is_none() {
            return Err(StudyError::InvalidNode(node_id));
        }
        tree.set_shapes(node_id, shapes);
        self.persist(study, &tree).await?;
        Ok(())
    }

    /// Promote a variation to the mainline (move it to the front of its parent's
    /// children) on a study the caller may write.
    pub async fn promote_variation(
        &self,
        user: &CurrentUser,
        study_id: i32,
        node_id: usize,
    ) -> Result<(), StudyError> {
        self.edit_tree(user, study_id, |tree| tree.promote(node_id))
            .await
    }

    /// Reorder a node among its siblings, moving it to `index` (0 = mainline) in
    /// its parent's child list, on a study the caller may write.
    pub async fn reorder_variation(
        &self,
        user: &CurrentUser,
        study_id: i32,
        node_id: usize,
        index: usize,
    ) -> Result<(), StudyError> {
        self.edit_tree(user, study_id, |tree| tree.reorder(node_id, index))
            .await
    }

    /// Delete a node and its whole subtree on a study the caller may write. The
    /// surviving tree is reindexed, so callers should reload it afterwards.
    pub async fn delete_node(
        &self,
        user: &CurrentUser,
        study_id: i32,
        node_id: usize,
    ) -> Result<(), StudyError> {
        self.edit_tree(user, study_id, |tree| tree.delete(node_id))
            .await
    }

    /// Load a writable study, deserialize its tree, apply a structural edit, and
    /// persist it. The shared body of the promote / reorder / delete operations.
    async fn edit_tree(
        &self,
        user: &CurrentUser,
        study_id: i32,
        edit: impl FnOnce(&mut MoveTree) -> Result<(), TreeError>,
    ) -> Result<(), StudyError> {
        let study = self.load_writable(user, study_id).await?;
        let mut tree: MoveTree = serde_json::from_str(&study.tree_json)?;
        edit(&mut tree)?;
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
        assert_can_write(study.owner_id.as_deref(), user).map_err(|_| StudyError::Forbidden)?;
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

/// FEN of the position reached by replaying a node's SAN line from the start.
fn fen_at(line: &[String]) -> Result<String, PositionError> {
    match replay(STARTPOS_FEN, line, MODE)?.last() {
        Some(ply) => Ok(ply.fen.clone()),
        None => Ok(STARTPOS_FEN.to_string()),
    }
}

/// Resolve a [`MoveInput`] to the canonical SAN stored in the tree, validating
/// legality in `fen`. SAN is matched against the generated legal-move set
/// (accepting a check/mate suffix); UCI is converted via shakmaty, which tolerates
/// the disambiguation SAN rejects. Either way an illegal move is an `IllegalMove`.
fn resolve_move(fen: &str, mv: &MoveInput) -> Result<String, StudyError> {
    match mv {
        MoveInput::San(san) => legal_sans(fen, MODE)?
            .into_iter()
            .find(|legal| san_core(legal) == san_core(san))
            .ok_or_else(|| StudyError::IllegalMove {
                san: san.clone(),
                fen: fen.to_string(),
            }),
        MoveInput::Uci(uci) => uci_to_san(fen, uci, MODE).map_err(|_| StudyError::IllegalMove {
            san: uci.clone(),
            fen: fen.to_string(),
        }),
    }
}

/// SAN without its trailing check/mate marker, so `Qh5+` matches the generated
/// `Qh5`.
fn san_core(san: &str) -> &str {
    san.trim_end_matches(['+', '#'])
}

#[cfg(test)]
mod tests;
