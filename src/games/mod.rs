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

use sea_orm::sea_query::{Expr, Func, SimpleExpr};
use sea_orm::{
    ColumnTrait, DatabaseConnection, DbErr, EntityTrait, Order, PaginatorTrait, QueryFilter,
    QueryOrder, QuerySelect, Select,
};
use serde::Serialize;

use crate::db::entities::{databases, games, players, position_index};
use crate::server::identity::{assert_can_write, scope, CurrentUser};

/// Default page size when the caller does not specify `limit`.
pub const DEFAULT_LIMIT: u64 = 50;
/// Upper bound on a single page, so an unbounded `limit` cannot load a database.
pub const MAX_LIMIT: u64 = 200;

/// The column the game list is ordered by. `id` is always the final tiebreaker
/// so every sort is a total order. Defaults to [`GameSort::Date`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum GameSort {
    /// PGN `Date` (NULLs coalesced to `""`). With `Desc` this is "latest first".
    #[default]
    Date,
    /// Game `Result` (`1-0` / `0-1` / `1/2-1/2` / `*`).
    Result,
    /// Opening `ECO` code.
    Eco,
    /// Insertion order (`games.id`).
    Added,
}

impl GameSort {
    /// Parse the `sort` query value. Unknown/blank falls back to the default
    /// (`Date`) — the field is a closed set the UI picks from, not free input.
    pub fn parse(value: Option<&str>) -> Self {
        match value.map(str::trim) {
            Some("result") => GameSort::Result,
            Some("eco") => GameSort::Eco,
            Some("added") | Some("id") => GameSort::Added,
            _ => GameSort::Date,
        }
    }
}

/// Sort direction for the chosen [`GameSort`] (and the `id` tiebreaker). Defaults
/// to [`SortDir::Desc`] so the date sort yields newest-first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SortDir {
    Asc,
    #[default]
    Desc,
}

impl SortDir {
    /// Parse the `dir` query value; anything but `asc` is `Desc` (the default).
    pub fn parse(value: Option<&str>) -> Self {
        match value.map(str::trim) {
            Some("asc") => SortDir::Asc,
            _ => SortDir::Desc,
        }
    }

    fn order(self) -> Order {
        match self {
            SortDir::Asc => Order::Asc,
            SortDir::Desc => Order::Desc,
        }
    }
}

/// A validated game-list request: which database, which page (0-based, `limit`
/// rows each), and how to sort. The HTTP route and MCP tool both build one.
#[derive(Debug, Clone)]
pub struct GameListParams {
    pub database_id: i32,
    pub page: u64,
    pub limit: u64,
    pub sort: GameSort,
    pub dir: SortDir,
}

/// Why a game operation failed. Transport-agnostic — the HTTP / MCP layer maps
/// each variant onto its own status / error envelope.
#[derive(Debug, thiserror::Error)]
pub enum GameError {
    /// No game (or database) with that id is visible to the caller.
    #[error("game not found")]
    NotFound,
    /// Authenticated but not permitted: a game in a global database deleted by a
    /// non-admin (mirrors [`crate::databases::DatabaseError::Forbidden`]).
    #[error("not permitted")]
    Forbidden,
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

/// One page of an offset-paginated game list, with `total` so the client can
/// render a real paginator (page count, "showing a–b of N"). `page`/`limit` echo
/// what was actually applied (limit after clamping).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GamePage {
    pub games: Vec<GameSummary>,
    pub total: u64,
    pub page: u64,
    pub limit: u64,
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

    /// One offset page of the games in `params.database_id`, sorted per
    /// `params.sort`/`dir` (default: date, newest-first), with the total game
    /// count for the paginator. `limit` is clamped to `[1, MAX_LIMIT]`; `page` is
    /// 0-based. The database must be visible to the caller, else `NotFound`
    /// (which also hides ids that don't exist at all).
    pub async fn list(
        &self,
        user: &CurrentUser,
        params: &GameListParams,
    ) -> Result<GamePage, GameError> {
        self.assert_database_visible(user, params.database_id)
            .await?;

        let limit = params.limit.clamp(1, MAX_LIMIT);
        let base = games::Entity::find().filter(games::Column::DatabaseId.eq(params.database_id));
        let total = base.clone().count(&self.db).await?;
        let rows = apply_order(base, params.sort, params.dir)
            .offset(params.page.saturating_mul(limit))
            .limit(limit)
            .all(&self.db)
            .await?;

        let names = player_names(&self.db, &rows).await?;
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
        Ok(GamePage {
            games,
            total,
            page: params.page,
            limit,
        })
    }

    /// A single game with its PGN, if it is visible to the caller.
    pub async fn get(&self, user: &CurrentUser, id: i32) -> Result<GameDetail, GameError> {
        let game = games::Entity::find_by_id(id)
            .one(&self.db)
            .await?
            .ok_or(GameError::NotFound)?;
        self.assert_database_visible(user, game.database_id).await?;

        let names = player_names(&self.db, std::slice::from_ref(&game)).await?;
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

    /// Delete a game the caller may write. Its owning database must be visible
    /// (own ∪ global) and writable (own, or admin for a global one) — the same
    /// write guard collections use (mirrors [`DatabaseService::delete`]). An id
    /// the caller cannot see is hidden as `NotFound`; a visible-but-unwritable
    /// database (a global one for a non-admin) is `Forbidden`.
    ///
    /// [`DatabaseService::delete`]: crate::databases::DatabaseService::delete
    pub async fn delete(&self, user: &CurrentUser, id: i32) -> Result<(), GameError> {
        let game = games::Entity::find_by_id(id)
            .one(&self.db)
            .await?
            .ok_or(GameError::NotFound)?;
        // Hide games in databases the caller cannot see before revealing more.
        self.assert_database_visible(user, game.database_id).await?;
        let database = databases::Entity::find_by_id(game.database_id)
            .one(&self.db)
            .await?
            .ok_or(GameError::NotFound)?;
        assert_can_write(database.owner_id.as_deref(), user).map_err(|_| GameError::Forbidden)?;
        // The Zobrist `position_index` rows reference this game (FK RESTRICT, no
        // cascade on SQLite), so drop them before the game row itself.
        position_index::Entity::delete_many()
            .filter(position_index::Column::GameId.eq(game.id))
            .exec(&self.db)
            .await?;
        games::Entity::delete_by_id(game.id).exec(&self.db).await?;
        Ok(())
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
}

/// `player_id -> name` for every player referenced by `games`, loaded in one
/// query. Shared by the game-listing, header-search and position-search services
/// so the roster lookup lives in one place; returns the raw [`DbErr`], which each
/// caller maps onto its own error type.
pub(crate) async fn player_names(
    db: &DatabaseConnection,
    games: &[games::Model],
) -> Result<HashMap<i32, String>, DbErr> {
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
        .all(db)
        .await?
        .into_iter()
        .collect())
}

/// Apply the chosen sort to `select`, always appending the `id` tiebreaker (in
/// the same direction) so the page boundary is deterministic. Text columns are
/// coalesced to `""` so NULLs get a defined slot and the order matches between
/// SQLite (local) and Postgres (server), which place NULLs differently.
fn apply_order(
    select: Select<games::Entity>,
    sort: GameSort,
    dir: SortDir,
) -> Select<games::Entity> {
    let order = dir.order();
    let key = match sort {
        GameSort::Added => return select.order_by(games::Column::Id, order),
        GameSort::Date => text_key(games::Column::Date),
        GameSort::Result => text_key(games::Column::Result),
        GameSort::Eco => text_key(games::Column::Eco),
    };
    select
        .order_by(key, order.clone())
        .order_by(games::Column::Id, order)
}

/// `COALESCE(col, '')` — a NULL-safe orderable key for a nullable text column.
fn text_key(col: games::Column) -> SimpleExpr {
    Func::coalesce([Expr::col(col).into(), Expr::val("").into()]).into()
}

pub mod export;
pub mod routes;

#[cfg(test)]
mod tests;
