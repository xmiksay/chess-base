//! Transport-agnostic game-listing service (issue #68): paginated listing of the
//! games in a database plus single-game fetch (with PGN movetext) for board
//! playback. The HTTP routes and the planned MCP tools are thin callers (mirrors
//! [`crate::databases::DatabaseService`] / [`crate::search::position`]).
//!
//! It carries no HTTP/MCP concerns: every method takes a [`CurrentUser`] and
//! returns plain data or a [`GameError`] the transport maps to a response.
//! Visibility follows the ownership rule (ADR 0007 / 0011): a game is visible
//! when its owning database is the caller's own or global (`owner_id IS NULL`).

use std::collections::{HashMap, HashSet};

use sea_orm::{
    ColumnTrait, DatabaseConnection, DbErr, EntityTrait, QueryFilter, QueryOrder, QuerySelect,
};
use serde::Serialize;

use crate::db::entities::{databases, games, players};
use crate::server::identity::{scope, CurrentUser};

/// Default page size when the caller does not specify `limit`.
pub const DEFAULT_LIMIT: u64 = 50;
/// Upper bound on a single page, so an unbounded `limit` cannot load a database.
pub const MAX_LIMIT: u64 = 200;

/// Why a game operation failed. Transport-agnostic — the HTTP / MCP layer maps
/// each variant onto its own status / error envelope.
#[derive(Debug, thiserror::Error)]
pub enum GameError {
    /// No game (or database) with that id is visible to the caller.
    #[error("game not found")]
    NotFound,
    /// Underlying database error (never surfaced verbatim to clients).
    #[error(transparent)]
    Db(#[from] DbErr),
}

/// A row in the game list: header roster with player names resolved for display.
/// Omits the (potentially large) PGN movetext, which [`GameDetail`] carries.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GameSummary {
    pub id: i32,
    pub database_id: i32,
    pub white: Option<String>,
    pub black: Option<String>,
    pub date: Option<String>,
    pub result: Option<String>,
    pub eco: Option<String>,
    pub white_elo: Option<i32>,
    pub black_elo: Option<i32>,
    pub ply_count: Option<i32>,
}

/// A single game with everything needed to replay it on the board: the PGN
/// movetext plus `variant`/`start_fen` for Chess960 and set-up positions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GameDetail {
    pub id: i32,
    pub database_id: i32,
    pub white: Option<String>,
    pub black: Option<String>,
    pub site: Option<String>,
    pub round: Option<String>,
    pub date: Option<String>,
    pub result: Option<String>,
    pub eco: Option<String>,
    pub white_elo: Option<i32>,
    pub black_elo: Option<i32>,
    pub variant: String,
    pub start_fen: Option<String>,
    pub ply_count: Option<i32>,
    pub pgn: Option<String>,
}

/// One page of a keyset-paginated game list. `next_cursor` is the id to pass as
/// `after` for the following page, or `None` when the last page was returned.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GamePage {
    pub games: Vec<GameSummary>,
    pub next_cursor: Option<i32>,
}

/// Game listing over the `games` table. Holds a connection handle (cheap to
/// clone — SeaORM wraps an `Arc`'d pool).
#[derive(Clone)]
pub struct GameService {
    db: DatabaseConnection,
}

impl GameService {
    pub fn new(db: DatabaseConnection) -> Self {
        Self { db }
    }

    /// One keyset page of the games in `database_id`, ordered oldest-first by id.
    /// `after` is the exclusive cursor (last id of the previous page); `limit` is
    /// clamped to `[1, MAX_LIMIT]`. The database must be visible to the caller,
    /// else `NotFound` (which also hides ids that don't exist at all).
    pub async fn list(
        &self,
        user: &CurrentUser,
        database_id: i32,
        after: Option<i32>,
        limit: Option<u64>,
    ) -> Result<GamePage, GameError> {
        self.assert_database_visible(user, database_id).await?;

        let limit = limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
        let mut query = games::Entity::find()
            .filter(games::Column::DatabaseId.eq(database_id))
            .order_by_asc(games::Column::Id)
            // Fetch one extra row to tell whether a further page exists.
            .limit(limit + 1);
        if let Some(after) = after {
            query = query.filter(games::Column::Id.gt(after));
        }
        let mut rows = query.all(&self.db).await?;

        let next_cursor = if rows.len() as u64 > limit {
            rows.truncate(limit as usize);
            rows.last().map(|g| g.id)
        } else {
            None
        };

        let names = self.player_names(&rows).await?;
        let games = rows
            .into_iter()
            .map(|g| GameSummary {
                white: g.white_player_id.and_then(|id| names.get(&id).cloned()),
                black: g.black_player_id.and_then(|id| names.get(&id).cloned()),
                id: g.id,
                database_id: g.database_id,
                date: g.date,
                result: g.result,
                eco: g.eco,
                white_elo: g.white_elo,
                black_elo: g.black_elo,
                ply_count: g.ply_count,
            })
            .collect();
        Ok(GamePage { games, next_cursor })
    }

    /// A single game with its PGN, if it is visible to the caller.
    pub async fn get(&self, user: &CurrentUser, id: i32) -> Result<GameDetail, GameError> {
        let game = games::Entity::find_by_id(id)
            .one(&self.db)
            .await?
            .ok_or(GameError::NotFound)?;
        self.assert_database_visible(user, game.database_id).await?;

        let names = self.player_names(std::slice::from_ref(&game)).await?;
        Ok(GameDetail {
            white: game.white_player_id.and_then(|id| names.get(&id).cloned()),
            black: game.black_player_id.and_then(|id| names.get(&id).cloned()),
            id: game.id,
            database_id: game.database_id,
            site: game.site,
            round: game.round,
            date: game.date,
            result: game.result,
            eco: game.eco,
            white_elo: game.white_elo,
            black_elo: game.black_elo,
            variant: game.variant,
            start_fen: game.start_fen,
            ply_count: game.ply_count,
            pgn: game.pgn,
        })
    }

    /// Ensure the database is visible to the caller (own ∪ global), else NotFound.
    async fn assert_database_visible(
        &self,
        user: &CurrentUser,
        database_id: i32,
    ) -> Result<(), GameError> {
        databases::Entity::find_by_id(database_id)
            .filter(scope(databases::Column::OwnerId, user))
            .one(&self.db)
            .await?
            .map(|_| ())
            .ok_or(GameError::NotFound)
    }

    /// `player_id -> name` for every player referenced by `games`.
    async fn player_names(
        &self,
        games: &[games::Model],
    ) -> Result<HashMap<i32, String>, GameError> {
        let ids: HashSet<i32> = games
            .iter()
            .flat_map(|g| [g.white_player_id, g.black_player_id])
            .flatten()
            .collect();
        if ids.is_empty() {
            return Ok(HashMap::new());
        }
        Ok(players::Entity::find()
            .filter(players::Column::Id.is_in(ids))
            .select_only()
            .column(players::Column::Id)
            .column(players::Column::Name)
            .into_tuple::<(i32, String)>()
            .all(&self.db)
            .await?
            .into_iter()
            .collect())
    }
}

pub mod routes;

#[cfg(test)]
mod tests;
