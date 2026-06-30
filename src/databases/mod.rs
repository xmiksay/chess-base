//! Transport-agnostic database (collection) service: the one place CRUD over the
//! `databases` table lives, so the HTTP routes and the planned MCP `/mcp` tools
//! are thin callers (mirrors [`crate::studies::StudyService`]).
//!
//! It carries no HTTP/MCP concerns: every method takes a [`CurrentUser`] and
//! returns plain models or a [`DatabaseError`] the transport maps to a response.
//! Ownership follows ADR 0007 / 0011 — reads see the caller's databases plus
//! global (`owner_id IS NULL`) ones; writes touch only the caller's own
//! databases, and a global database requires admin.

use std::collections::HashMap;

use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, DbErr, EntityTrait, QueryFilter, QueryOrder,
    QuerySelect, Set,
};

use crate::db::entities::{databases, games};
use crate::server::identity::{assert_admin, assert_can_write, scope, CurrentUser};

/// Why a database operation failed. Transport-agnostic — the HTTP / MCP layer
/// maps each variant onto its own status / error envelope.
#[derive(Debug, thiserror::Error)]
pub enum DatabaseError {
    /// No database with that id is visible to the caller.
    #[error("database not found")]
    NotFound,
    /// Authenticated but not permitted: another user's database, or a global
    /// database touched by a non-admin.
    #[error("not permitted")]
    Forbidden,
    /// `kind` is not one of the four known collection kinds.
    #[error("invalid kind '{0}' (expected one of lichess, chesscom, master, own)")]
    InvalidKind(String),
    /// A required field was blank (e.g. an empty/whitespace name).
    #[error("{0}")]
    InvalidInput(String),
    /// Underlying database error (never surfaced verbatim to clients).
    #[error(transparent)]
    Db(#[from] DbErr),
}

/// Database CRUD over the `databases` table. Holds a connection handle (cheap to
/// clone — SeaORM wraps an `Arc`'d pool).
#[derive(Clone)]
pub struct DatabaseService {
    db: DatabaseConnection,
}

impl DatabaseService {
    pub fn new(db: DatabaseConnection) -> Self {
        Self { db }
    }

    /// Create a collection. `global` makes it an admin-owned database visible to
    /// everyone (`owner_id IS NULL`) and requires admin; otherwise it belongs to
    /// the caller. `index_depth` is derived from `kind` (ADR-0003).
    pub async fn create(
        &self,
        user: &CurrentUser,
        name: &str,
        kind: &str,
        global: bool,
    ) -> Result<databases::Model, DatabaseError> {
        let name = validate_name(name)?;
        if !databases::is_valid_kind(kind) {
            return Err(DatabaseError::InvalidKind(kind.to_string()));
        }
        let owner_id = if global {
            assert_admin(user).map_err(|_| DatabaseError::Forbidden)?;
            None
        } else {
            Some(user.id.clone())
        };
        let model = databases::ActiveModel {
            owner_id: Set(owner_id),
            name: Set(name),
            kind: Set(kind.to_string()),
            index_depth: Set(databases::default_index_depth(kind)),
            ..Default::default()
        }
        .insert(&self.db)
        .await?;
        Ok(model)
    }

    /// All databases visible to the caller (own ∪ global), oldest first.
    pub async fn list(&self, user: &CurrentUser) -> Result<Vec<databases::Model>, DatabaseError> {
        let rows = databases::Entity::find()
            .filter(scope(databases::Column::OwnerId, user))
            .order_by_asc(databases::Column::Id)
            .all(&self.db)
            .await?;
        Ok(rows)
    }

    /// All databases visible to the caller paired with their game counts, in one
    /// extra grouped query. Powers the MCP `list_databases` tool (issue #125), so
    /// an agent can both discover a `database_id` and see how populated each
    /// collection is before building a study against it.
    pub async fn list_with_counts(
        &self,
        user: &CurrentUser,
    ) -> Result<Vec<(databases::Model, i64)>, DatabaseError> {
        let rows = self.list(user).await?;
        let ids: Vec<i32> = rows.iter().map(|d| d.id).collect();
        let counts = game_counts(&self.db, &ids).await?;
        Ok(rows
            .into_iter()
            .map(|d| {
                let n = counts.get(&d.id).copied().unwrap_or(0);
                (d, n)
            })
            .collect())
    }

    /// A single database, if it is visible to the caller.
    pub async fn get(
        &self,
        user: &CurrentUser,
        id: i32,
    ) -> Result<databases::Model, DatabaseError> {
        databases::Entity::find_by_id(id)
            .filter(scope(databases::Column::OwnerId, user))
            .one(&self.db)
            .await?
            .ok_or(DatabaseError::NotFound)
    }

    /// Rename a database the caller may write. Returns the updated row.
    pub async fn rename(
        &self,
        user: &CurrentUser,
        id: i32,
        name: &str,
    ) -> Result<databases::Model, DatabaseError> {
        let name = validate_name(name)?;
        let model = self.load_writable(user, id).await?;
        let mut active: databases::ActiveModel = model.into();
        active.name = Set(name);
        Ok(active.update(&self.db).await?)
    }

    /// Delete a database the caller may write.
    pub async fn delete(&self, user: &CurrentUser, id: i32) -> Result<(), DatabaseError> {
        let model = self.load_writable(user, id).await?;
        databases::Entity::delete_by_id(model.id)
            .exec(&self.db)
            .await?;
        Ok(())
    }

    /// Load a database by id and enforce the write guard: the caller must own it,
    /// or be admin for a global one. `NotFound` hides ids that don't exist at all.
    async fn load_writable(
        &self,
        user: &CurrentUser,
        id: i32,
    ) -> Result<databases::Model, DatabaseError> {
        let model = databases::Entity::find_by_id(id)
            .one(&self.db)
            .await?
            .ok_or(DatabaseError::NotFound)?;
        assert_can_write(model.owner_id.as_deref(), user).map_err(|_| DatabaseError::Forbidden)?;
        Ok(model)
    }
}

/// `database_id -> game count` for the given database ids, in a single grouped
/// query. Empty `ids` short-circuits without touching the DB; databases with no
/// games are simply absent from the map (the caller defaults them to `0`).
async fn game_counts(db: &DatabaseConnection, ids: &[i32]) -> Result<HashMap<i32, i64>, DbErr> {
    if ids.is_empty() {
        return Ok(HashMap::new());
    }
    let rows = games::Entity::find()
        .filter(games::Column::DatabaseId.is_in(ids.iter().copied()))
        .select_only()
        .column(games::Column::DatabaseId)
        .column_as(games::Column::Id.count(), "count")
        .group_by(games::Column::DatabaseId)
        .into_tuple::<(i32, i64)>()
        .all(db)
        .await?;
    Ok(rows.into_iter().collect())
}

/// Trim and reject a blank name.
fn validate_name(name: &str) -> Result<String, DatabaseError> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err(DatabaseError::InvalidInput("name must not be empty".into()));
    }
    Ok(trimmed.to_string())
}

pub mod routes;

#[cfg(test)]
mod tests;
