//! Transport-agnostic folder service: study organization in an adjacency-list
//! directory tree (issue #164, ADR-0030). The HTTP routes are thin callers; all
//! ownership/admin gating and the cycle/cascade rules live here.
//!
//! Ownership follows ADR 0007 / 0011 — reads see the caller's folders plus global
//! (`owner_id IS NULL`) ones; writes touch only the caller's own folders, and a
//! global folder requires admin. The tree is account-level, independent of game
//! databases.
//!
//! Referential rules are enforced here (not by DB FKs): SQLite has foreign keys
//! off by default and cannot ALTER-add one, so [`FolderService::delete`] cascades
//! child folders and nulls the `folder_id` of every contained study itself.

pub mod routes;

use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, DbErr, EntityTrait, QueryFilter, QueryOrder,
    RuntimeErr, Set,
};

use crate::db::entities::{folders, studies};
use crate::server::identity::{assert_admin, assert_can_write, scope, CurrentUser};

/// Why a folder operation failed. Transport-agnostic — the HTTP / MCP layer maps
/// each variant onto its own status / error envelope.
#[derive(Debug, thiserror::Error)]
pub enum FolderError {
    /// No folder with that id is visible to the caller.
    #[error("folder not found")]
    NotFound,
    /// Authenticated but not permitted: another user's folder, or a global folder
    /// touched by a non-admin.
    #[error("not permitted")]
    Forbidden,
    /// A move would place a folder inside its own subtree (or into itself).
    #[error("a folder cannot be moved into its own descendant")]
    Cycle,
    /// A sibling folder with that name already exists (unique constraint).
    #[error("a folder with that name already exists here")]
    Duplicate,
    /// Underlying database error (never surfaced verbatim to clients).
    #[error(transparent)]
    Db(#[from] DbErr),
}

/// Folder CRUD + tree maintenance over the `folders` table. Holds a connection
/// handle (cheap to clone — SeaORM wraps an `Arc`'d pool).
#[derive(Clone)]
pub struct FolderService {
    db: DatabaseConnection,
}

impl FolderService {
    pub fn new(db: DatabaseConnection) -> Self {
        Self { db }
    }

    /// All folders visible to the caller (own ∪ global), ordered for a stable
    /// tree render (by name, then id). The caller assembles the hierarchy from the
    /// flat list via each row's `parent_id`.
    pub async fn list(&self, user: &CurrentUser) -> Result<Vec<folders::Model>, FolderError> {
        let rows = folders::Entity::find()
            .filter(scope(folders::Column::OwnerId, user))
            .order_by_asc(folders::Column::Name)
            .order_by_asc(folders::Column::Id)
            .all(&self.db)
            .await?;
        Ok(rows)
    }

    /// Create a folder. `global` makes it an admin-owned folder visible to
    /// everyone (`owner_id IS NULL`) and requires admin; otherwise it belongs to
    /// the caller. A `parent` must be visible to and owned the same way as the new
    /// folder (no mixing an own folder under a global one or vice versa).
    pub async fn create(
        &self,
        user: &CurrentUser,
        parent: Option<i32>,
        name: impl Into<String>,
        global: bool,
    ) -> Result<folders::Model, FolderError> {
        let owner_id = if global {
            assert_admin(user).map_err(|_| FolderError::Forbidden)?;
            None
        } else {
            Some(user.id.clone())
        };
        if let Some(parent_id) = parent {
            self.assert_parent_compatible(user, parent_id, owner_id.as_deref())
                .await?;
        }
        let name = name.into();
        self.assert_unique_sibling(owner_id.as_deref(), parent, &name, None)
            .await?;
        let model = folders::ActiveModel {
            owner_id: Set(owner_id),
            parent_id: Set(parent),
            name: Set(name),
            ..Default::default()
        }
        .insert(&self.db)
        .await
        .map_err(map_unique)?;
        Ok(model)
    }

    /// Rename a folder the caller may write. Returns the updated row.
    pub async fn rename(
        &self,
        user: &CurrentUser,
        id: i32,
        name: impl Into<String>,
    ) -> Result<folders::Model, FolderError> {
        let folder = self.load_writable(user, id).await?;
        let name = name.into();
        self.assert_unique_sibling(
            folder.owner_id.as_deref(),
            folder.parent_id,
            &name,
            Some(id),
        )
        .await?;
        let mut active: folders::ActiveModel = folder.into();
        active.name = Set(name);
        active.update(&self.db).await.map_err(map_unique)
    }

    /// Reparent a folder the caller may write (`None` ⇒ move to root). Rejects a
    /// move into the folder itself or any of its descendants (a cycle), and a
    /// target owned differently than the folder.
    pub async fn reparent(
        &self,
        user: &CurrentUser,
        id: i32,
        new_parent: Option<i32>,
    ) -> Result<folders::Model, FolderError> {
        let folder = self.load_writable(user, id).await?;
        if let Some(parent_id) = new_parent {
            if parent_id == id {
                return Err(FolderError::Cycle);
            }
            self.assert_parent_compatible(user, parent_id, folder.owner_id.as_deref())
                .await?;
            if self.is_descendant(&folder, parent_id).await? {
                return Err(FolderError::Cycle);
            }
        }
        self.assert_unique_sibling(
            folder.owner_id.as_deref(),
            new_parent,
            &folder.name,
            Some(id),
        )
        .await?;
        let mut active: folders::ActiveModel = folder.into();
        active.parent_id = Set(new_parent);
        active.update(&self.db).await.map_err(map_unique)
    }

    /// Delete a folder the caller may write, cascading to its whole subtree of
    /// child folders. Studies contained anywhere in that subtree are unfiled
    /// (`folder_id = NULL`), never deleted. Enforced here because SQLite does not
    /// fire the FK cascade.
    pub async fn delete(&self, user: &CurrentUser, id: i32) -> Result<(), FolderError> {
        let folder = self.load_writable(user, id).await?;
        let subtree = self.subtree_ids(&folder).await?;

        // Unfile every study under the removed subtree (ON DELETE SET NULL).
        let unfiled = studies::ActiveModel {
            folder_id: Set(None),
            ..Default::default()
        };
        studies::Entity::update_many()
            .set(unfiled)
            .filter(studies::Column::FolderId.is_in(subtree.clone()))
            .exec(&self.db)
            .await?;

        folders::Entity::delete_many()
            .filter(folders::Column::Id.is_in(subtree))
            .exec(&self.db)
            .await?;
        Ok(())
    }

    /// Load a folder by id and enforce the write guard: the caller must own it, or
    /// be admin for a global one. `NotFound` hides ids that don't exist at all.
    async fn load_writable(
        &self,
        user: &CurrentUser,
        id: i32,
    ) -> Result<folders::Model, FolderError> {
        let folder = folders::Entity::find_by_id(id)
            .one(&self.db)
            .await?
            .ok_or(FolderError::NotFound)?;
        assert_can_write(folder.owner_id.as_deref(), user).map_err(|_| FolderError::Forbidden)?;
        Ok(folder)
    }

    /// Reject a duplicate sibling name. The DB unique index is a backstop only —
    /// it can't catch root-level (`parent_id IS NULL`) clashes because NULL is
    /// distinct in a unique index on both backends — so the check lives here too.
    /// `exclude` skips the folder being renamed/moved (it may keep its own name).
    async fn assert_unique_sibling(
        &self,
        owner_id: Option<&str>,
        parent_id: Option<i32>,
        name: &str,
        exclude: Option<i32>,
    ) -> Result<(), FolderError> {
        let owner_filter = match owner_id {
            Some(owner) => folders::Column::OwnerId.eq(owner.to_string()),
            None => folders::Column::OwnerId.is_null(),
        };
        let parent_filter = match parent_id {
            Some(parent) => folders::Column::ParentId.eq(parent),
            None => folders::Column::ParentId.is_null(),
        };
        let clash = folders::Entity::find()
            .filter(owner_filter)
            .filter(parent_filter)
            .filter(folders::Column::Name.eq(name.to_string()))
            .all(&self.db)
            .await?
            .into_iter()
            .any(|f| Some(f.id) != exclude);
        if clash {
            return Err(FolderError::Duplicate);
        }
        Ok(())
    }

    /// Ensure a prospective parent is visible to the caller and owned the same way
    /// as the child (so an owner's tree and the global tree never interleave).
    async fn assert_parent_compatible(
        &self,
        user: &CurrentUser,
        parent_id: i32,
        child_owner: Option<&str>,
    ) -> Result<(), FolderError> {
        let parent = folders::Entity::find_by_id(parent_id)
            .filter(scope(folders::Column::OwnerId, user))
            .one(&self.db)
            .await?
            .ok_or(FolderError::NotFound)?;
        if parent.owner_id.as_deref() != child_owner {
            return Err(FolderError::Forbidden);
        }
        Ok(())
    }

    /// Whether `candidate` is `folder` itself or sits inside its subtree — i.e.
    /// reparenting `folder` under `candidate` would form a cycle. Walks the parent
    /// chain upward from `candidate`; bounded by the loaded folder set.
    async fn is_descendant(
        &self,
        folder: &folders::Model,
        candidate: i32,
    ) -> Result<bool, FolderError> {
        let owned = self.owned_folders(folder.owner_id.as_deref()).await?;
        let mut cursor = Some(candidate);
        while let Some(id) = cursor {
            if id == folder.id {
                return Ok(true);
            }
            cursor = owned.iter().find(|f| f.id == id).and_then(|f| f.parent_id);
        }
        Ok(false)
    }

    /// The ids of `folder` and every folder beneath it (a downward BFS over the
    /// owner's folder set).
    async fn subtree_ids(&self, folder: &folders::Model) -> Result<Vec<i32>, FolderError> {
        let owned = self.owned_folders(folder.owner_id.as_deref()).await?;
        let mut ids = vec![folder.id];
        let mut frontier = vec![folder.id];
        while let Some(parent) = frontier.pop() {
            for child in owned.iter().filter(|f| f.parent_id == Some(parent)) {
                ids.push(child.id);
                frontier.push(child.id);
            }
        }
        Ok(ids)
    }

    /// Every folder sharing an `owner_id` (the same scope a move/delete stays
    /// within), loaded once so tree walks don't issue a query per node.
    async fn owned_folders(
        &self,
        owner_id: Option<&str>,
    ) -> Result<Vec<folders::Model>, FolderError> {
        let filter = match owner_id {
            Some(owner) => folders::Column::OwnerId.eq(owner.to_string()),
            None => folders::Column::OwnerId.is_null(),
        };
        Ok(folders::Entity::find().filter(filter).all(&self.db).await?)
    }
}

/// Map a unique-index violation (duplicate sibling name) onto [`FolderError::Duplicate`];
/// any other DB error passes through unchanged.
fn map_unique(err: DbErr) -> FolderError {
    let text = match &err {
        DbErr::Query(RuntimeErr::SqlxError(e)) => e.to_string(),
        DbErr::Exec(RuntimeErr::SqlxError(e)) => e.to_string(),
        other => other.to_string(),
    };
    let lower = text.to_ascii_lowercase();
    if lower.contains("unique") || lower.contains("duplicate") {
        FolderError::Duplicate
    } else {
        FolderError::Db(err)
    }
}

#[cfg(test)]
mod tests;
