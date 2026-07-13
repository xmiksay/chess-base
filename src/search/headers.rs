//! Transport-agnostic header/metadata search (issue #6): query games by player /
//! color / event / ECO / date range / result, scoped to the caller's databases ∪
//! global ones (ADR 0007 / 0011), returned in stable keyset-paginated pages.
//!
//! Keyset (seek) pagination avoids `OFFSET`'s linear scan: each page is bounded
//! by the composite sort key of the last row it returned — the primary sort
//! column plus the unique `id` tiebreaker — so page N+1 is one indexed range
//! scan no matter how deep it sits. The opaque `cursor` is the base64url of that
//! key; the client echoes it back to advance.

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use sea_orm::sea_query::{Expr, Func, IntoCondition, SimpleExpr};
use sea_orm::{
    ColumnTrait, Condition, DatabaseConnection, DbErr, EntityTrait, Order, QueryFilter, QueryOrder,
    QuerySelect,
};
use serde::{Deserialize, Serialize};

use crate::db::entities::{events, games};
use crate::search::position::{Color, GameHit, PositionSearchService, SearchError};
use crate::server::identity::CurrentUser;

/// Page size when the caller omits `limit`.
const DEFAULT_LIMIT: u64 = 50;
/// Hard cap on page size, so one request can't ask for an unbounded scan.
const MAX_LIMIT: u64 = 200;

/// Why a header search failed. Transport-agnostic — the HTTP / MCP layer maps
/// each variant onto its own status / error envelope.
#[derive(Debug, thiserror::Error)]
pub enum HeaderSearchError {
    /// A malformed filter/sort parameter (unknown sort field, bad `color`, …).
    #[error("invalid query: {0}")]
    BadRequest(String),
    /// The supplied pagination cursor was not produced by a prior page.
    #[error("invalid cursor")]
    InvalidCursor,
    /// Serializing a result row / cursor failed (effectively unreachable for the
    /// flat types; kept so the transport never has to `unwrap`).
    #[error("serialization error")]
    Serialize(#[from] serde_json::Error),
    /// Underlying database error (never surfaced verbatim to clients).
    #[error(transparent)]
    Db(#[from] DbErr),
}

/// The column the result set is ordered by. `id` is always the tiebreaker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortField {
    /// PGN `Date`, with NULLs coalesced to the empty string for a total order.
    Date,
    /// Insertion order (`games.id`).
    Id,
}

/// Sort direction for the chosen [`SortField`] (and the `id` tiebreaker).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortDir {
    Asc,
    Desc,
}

impl SortDir {
    fn order(self) -> Order {
        match self {
            SortDir::Asc => Order::Asc,
            SortDir::Desc => Order::Desc,
        }
    }
}

/// The decoded position of the last row of a page: the primary sort value (the
/// coalesced date for [`SortField::Date`], absent for [`SortField::Id`]) plus the
/// `id` tiebreaker. Round-trips through base64url-JSON as the opaque cursor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Cursor {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    d: Option<String>,
    id: i32,
}

impl Cursor {
    fn encode(&self) -> Result<String, serde_json::Error> {
        Ok(URL_SAFE_NO_PAD.encode(serde_json::to_vec(self)?))
    }

    fn decode(s: &str) -> Result<Self, HeaderSearchError> {
        let bytes = URL_SAFE_NO_PAD
            .decode(s)
            .map_err(|_| HeaderSearchError::InvalidCursor)?;
        serde_json::from_slice(&bytes).map_err(|_| HeaderSearchError::InvalidCursor)
    }
}

/// Raw query parameters as received over the wire. Validated into a
/// [`HeaderQuery`] via [`TryFrom`]; blank strings are treated as "unset".
#[derive(Debug, Default, Deserialize)]
pub struct HeaderParams {
    pub player: Option<String>,
    pub color: Option<String>,
    pub event: Option<String>,
    pub eco: Option<String>,
    pub date_from: Option<String>,
    pub date_to: Option<String>,
    pub result: Option<String>,
    pub sort: Option<String>,
    pub dir: Option<String>,
    pub limit: Option<u64>,
    pub cursor: Option<String>,
}

/// A validated header search request: the parsed filters, sort, page size and
/// (optional) keyset cursor that [`HeaderSearchService::search`] executes.
#[derive(Debug, Clone)]
pub struct HeaderQuery {
    pub player: Option<String>,
    pub color: Option<Color>,
    pub event: Option<String>,
    pub eco: Option<String>,
    pub date_from: Option<String>,
    pub date_to: Option<String>,
    pub result: Option<String>,
    pub sort: SortField,
    pub dir: SortDir,
    pub limit: u64,
    cursor: Option<Cursor>,
}

impl TryFrom<HeaderParams> for HeaderQuery {
    type Error = HeaderSearchError;

    fn try_from(p: HeaderParams) -> Result<Self, Self::Error> {
        let color = match norm(p.color).as_deref() {
            None => None,
            Some("white") => Some(Color::White),
            Some("black") => Some(Color::Black),
            Some(other) => {
                return Err(HeaderSearchError::BadRequest(format!(
                    "color must be 'white' or 'black', got '{other}'"
                )))
            }
        };
        let sort = match norm(p.sort).as_deref() {
            None | Some("date") => SortField::Date,
            Some("id") => SortField::Id,
            Some(other) => {
                return Err(HeaderSearchError::BadRequest(format!(
                    "sort must be 'date' or 'id', got '{other}'"
                )))
            }
        };
        let dir = match norm(p.dir).as_deref() {
            None | Some("desc") => SortDir::Desc,
            Some("asc") => SortDir::Asc,
            Some(other) => {
                return Err(HeaderSearchError::BadRequest(format!(
                    "dir must be 'asc' or 'desc', got '{other}'"
                )))
            }
        };
        let limit = p.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
        let cursor = match norm(p.cursor) {
            Some(c) => Some(Cursor::decode(&c)?),
            None => None,
        };
        Ok(HeaderQuery {
            player: norm(p.player),
            color,
            event: norm(p.event),
            eco: norm(p.eco),
            date_from: norm(p.date_from),
            date_to: norm(p.date_to),
            result: norm(p.result),
            sort,
            dir,
            limit,
            cursor,
        })
    }
}

/// One page of header search results plus the cursor to fetch the next page
/// (`None` once the result set is exhausted).
#[derive(Debug, Serialize)]
pub struct HeaderPage {
    pub games: Vec<GameHit>,
    pub next_cursor: Option<String>,
}

/// Header/metadata search over the `games` table. Holds a connection handle
/// (cheap to clone — SeaORM wraps an `Arc`'d pool).
#[derive(Clone)]
pub struct HeaderSearchService {
    db: DatabaseConnection,
}

impl HeaderSearchService {
    pub fn new(db: DatabaseConnection) -> Self {
        Self { db }
    }

    /// Execute `query` for `user`, returning one keyset-paginated page. Scope is
    /// applied here (own ∪ global databases); a filter that resolves to nothing
    /// (e.g. an unknown player) short-circuits to an empty page.
    pub async fn search(
        &self,
        user: &CurrentUser,
        query: &HeaderQuery,
    ) -> Result<HeaderPage, HeaderSearchError> {
        // Scope is the caller's own ∪ global databases; reuse position search's
        // canonical lookup rather than re-deriving it here.
        let visible = PositionSearchService::new(self.db.clone())
            .visible_database_ids(user)
            .await
            .map_err(|e| match e {
                SearchError::Db(e) => HeaderSearchError::Db(e),
                SearchError::InvalidFen(m) | SearchError::BadRequest(m) => {
                    HeaderSearchError::BadRequest(m)
                }
                SearchError::Serialize(e) => HeaderSearchError::Serialize(e),
            })?;
        if visible.is_empty() {
            return Ok(HeaderPage::empty());
        }
        let mut cond = Condition::all().add(games::Column::DatabaseId.is_in(visible));

        if let Some(name) = &query.player {
            let ids = crate::search::position::player_ids_matching(&self.db, name)
                .await
                .map_err(HeaderSearchError::Db)?;
            if ids.is_empty() {
                return Ok(HeaderPage::empty());
            }
            cond = cond.add(match query.color {
                Some(Color::White) => games::Column::WhitePlayerId.is_in(ids).into_condition(),
                Some(Color::Black) => games::Column::BlackPlayerId.is_in(ids).into_condition(),
                None => Condition::any()
                    .add(games::Column::WhitePlayerId.is_in(ids.clone()))
                    .add(games::Column::BlackPlayerId.is_in(ids)),
            });
        }
        if let Some(name) = &query.event {
            let ids = self.event_ids_matching(name).await?;
            if ids.is_empty() {
                return Ok(HeaderPage::empty());
            }
            cond = cond.add(games::Column::EventId.is_in(ids));
        }
        if let Some(eco) = &query.eco {
            cond = cond.add(games::Column::Eco.starts_with(eco));
        }
        if let Some(from) = &query.date_from {
            cond = cond.add(games::Column::Date.gte(from.clone()));
        }
        if let Some(to) = &query.date_to {
            cond = cond.add(games::Column::Date.lte(to.clone()));
        }
        if let Some(result) = &query.result {
            cond = cond.add(games::Column::Result.eq(result.clone()));
        }
        if let Some(cursor) = &query.cursor {
            cond = cond.add(keyset(query.sort, query.dir, cursor));
        }

        // Fetch one extra row to learn whether a further page exists without a
        // second count query.
        let order = query.dir.order();
        let mut select = games::Entity::find().filter(cond);
        select = match query.sort {
            SortField::Id => select.order_by(games::Column::Id, order),
            SortField::Date => select
                .order_by(date_key(), order.clone())
                .order_by(games::Column::Id, order),
        };
        let mut rows = select.limit(query.limit + 1).all(&self.db).await?;

        let has_more = rows.len() as u64 > query.limit;
        rows.truncate(query.limit as usize);
        let next_cursor = if has_more {
            match rows.last() {
                Some(last) => Some(cursor_for(query.sort, last).encode()?),
                None => None,
            }
        } else {
            None
        };

        let names = crate::games::player_names(&self.db, &rows).await?;
        let games = rows
            .into_iter()
            .map(|g| GameHit::from_model(g, &names))
            .collect();
        Ok(HeaderPage { games, next_cursor })
    }

    /// Event ids whose name contains `name`. Events have no position-search
    /// equivalent to move to, so this reuses position search's `LIKE`-escaping
    /// helper (issue #172) rather than keeping a second copy here.
    async fn event_ids_matching(&self, name: &str) -> Result<Vec<i32>, HeaderSearchError> {
        Ok(events::Entity::find()
            .filter(events::Column::Name.like(crate::search::position::contains_like(name)))
            .select_only()
            .column(events::Column::Id)
            .into_tuple()
            .all(&self.db)
            .await?)
    }
}

impl HeaderPage {
    fn empty() -> Self {
        HeaderPage {
            games: Vec::new(),
            next_cursor: None,
        }
    }
}

/// Trim a parameter and treat a blank value as unset.
fn norm(s: Option<String>) -> Option<String> {
    s.map(|v| v.trim().to_string()).filter(|v| !v.is_empty())
}

/// The orderable primary key for date sorts: `COALESCE(date, '')`, giving NULL
/// dates a defined position so the keyset comparison stays a total order.
fn date_key() -> SimpleExpr {
    Func::coalesce([Expr::col(games::Column::Date).into(), Expr::val("").into()]).into()
}

/// The keyset of `row` under `sort` — the value a `next_cursor` carries.
fn cursor_for(sort: SortField, row: &games::Model) -> Cursor {
    match sort {
        SortField::Id => Cursor {
            d: None,
            id: row.id,
        },
        SortField::Date => Cursor {
            d: Some(row.date.clone().unwrap_or_default()),
            id: row.id,
        },
    }
}

/// Seek predicate excluding everything up to and including `cursor`, in the
/// chosen sort order: strictly past the primary value, or tied on it and past
/// the `id` tiebreaker.
fn keyset(sort: SortField, dir: SortDir, cursor: &Cursor) -> Condition {
    let (past_id, past_primary): (SimpleExpr, fn(SimpleExpr, String) -> SimpleExpr) = match dir {
        SortDir::Asc => (games::Column::Id.gt(cursor.id), |e, v| Expr::expr(e).gt(v)),
        SortDir::Desc => (games::Column::Id.lt(cursor.id), |e, v| Expr::expr(e).lt(v)),
    };
    match sort {
        SortField::Id => past_id.into_condition(),
        SortField::Date => {
            let d = cursor.d.clone().unwrap_or_default();
            Condition::any()
                .add(past_primary(date_key(), d.clone()))
                .add(
                    Condition::all()
                        .add(Expr::expr(date_key()).eq(d))
                        .add(past_id),
                )
        }
    }
}

#[cfg(test)]
#[path = "headers_tests.rs"]
mod tests;
