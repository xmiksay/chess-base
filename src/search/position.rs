//! Transport-agnostic position search (ADR-0003): the headline "find games
//! reaching this position" plus the opening tree of aggregated per-continuation
//! statistics, both keyed on the 64-bit Zobrist hash from [`crate::position`].
//!
//! Like [`crate::databases::DatabaseService`] it carries no HTTP/MCP concerns:
//! every method takes a [`CurrentUser`] and returns plain data or a
//! [`SearchError`] the transport maps to a response. Search scope follows the
//! ownership rule (ADR 0007 / 0011): a query sees the caller's databases plus
//! global (`owner_id IS NULL`) ones, filtered on the denormalized
//! `position_index.database_id` so no join to `games` is needed for scoping.

use std::collections::HashMap;

use sea_orm::sea_query::{IntoCondition, LikeExpr};
use sea_orm::{
    ColumnTrait, Condition, DatabaseConnection, DbErr, EntityTrait, JoinType, QueryFilter,
    QueryOrder, QuerySelect, QueryTrait, RelationTrait,
};
use serde::Serialize;

use crate::db::entities::{databases, games, players, position_index};
use crate::position::{zobrist_of_fen, CastlingMode};
use crate::server::identity::{scope, CurrentUser};

/// Restrict a player filter to one side of the board.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Color {
    White,
    Black,
}

/// Why a position search failed. Transport-agnostic — the HTTP / MCP layer maps
/// each variant onto its own status / error envelope.
#[derive(Debug, thiserror::Error)]
pub enum SearchError {
    /// The supplied FEN could not be parsed into a legal position.
    #[error("invalid FEN: {0}")]
    InvalidFen(String),
    /// A malformed filter parameter (e.g. an unrecognised `color`, issue #172).
    #[error("invalid query: {0}")]
    BadRequest(String),
    /// Serializing a result row to NDJSON failed (effectively unreachable for the
    /// flat result types; kept so the transport never has to `unwrap`).
    #[error("serialization error")]
    Serialize(#[from] serde_json::Error),
    /// Underlying database error (never surfaced verbatim to clients).
    #[error(transparent)]
    Db(#[from] DbErr),
}

/// One continuation played from the queried position, with its game outcomes
/// aggregated. `count` is the number of distinct games that played this move;
/// `white`/`draws`/`black` break those down by game result (`1-0` / `1/2-1/2` /
/// `0-1`). Games with an unknown result (`*`) count toward `count` only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MoveStat {
    /// SAN of the continuation (e.g. `Nf3`).
    pub san: String,
    pub count: u64,
    pub white: u64,
    pub draws: u64,
    pub black: u64,
}

/// Optional player/color/date-range filter applied to [`PositionSearchService::opening_tree`]
/// and [`PositionSearchService::games_with_position`] (issue #172): narrows the
/// continuations/games to one player's games (either side, or restricted to
/// `color`), a date range, or both — e.g. "what does Carlsen play here as
/// White". Filters via a `games` condition joined/subqueried in the same query
/// as the position lookup (issue #153: no id round-trips). All-`None` (the
/// `Default`) is a no-op. Mirrors header search's semantics (`search::headers`):
/// `color` only narrows when `player` is set — a color with no player is
/// meaningless (every game already has both a White and Black side) and is
/// silently ignored, exactly like `HeaderSearchService::search` today.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PositionFilter {
    pub player: Option<String>,
    pub color: Option<Color>,
    pub date_from: Option<String>,
    pub date_to: Option<String>,
}

/// Player ids whose name contains `name` (case-insensitive on SQLite's ASCII
/// `LIKE`); the substring match keeps player search forgiving of full names.
/// Shared by header search and position search (issue #172).
pub(crate) async fn player_ids_matching(
    db: &DatabaseConnection,
    name: &str,
) -> Result<Vec<i32>, DbErr> {
    players::Entity::find()
        .filter(players::Column::Name.like(contains_like(name)))
        .select_only()
        .column(players::Column::Id)
        .into_tuple()
        .all(db)
        .await
}

/// Build a substring `LIKE` pattern that matches `needle` literally. SeaORM's
/// `contains` wraps the raw input in `%…%` without escaping, so a `%`/`_` in a
/// player/event name (or a `%` query) would act as a wildcard and over-match
/// (issue #99). We escape the LIKE metacharacters with `\` and pair the pattern
/// with `ESCAPE '\'` so they match as literals on both SQLite and Postgres.
/// `pub(crate)` so header search's event-name lookup (which has no position-search
/// equivalent to move to) can reuse it too.
pub(crate) fn contains_like(needle: &str) -> LikeExpr {
    LikeExpr::new(format!("%{}%", escape_like(needle))).escape('\\')
}

/// Escape the SQL `LIKE` metacharacters (`%`, `_`) and the escape char itself so
/// the result matches `s` verbatim under `ESCAPE '\'`. Backslash is escaped first
/// to avoid double-escaping the sequences introduced for `%`/`_`.
fn escape_like(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

/// A game reaching the queried position, with player names resolved for display.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GameHit {
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
    pub ply_count: Option<i32>,
}

impl GameHit {
    /// Project a game row to a [`GameHit`], resolving player ids to names. Shared
    /// by header and position search so the mapping lives in one place.
    pub(crate) fn from_model(g: games::Model, names: &HashMap<i32, String>) -> Self {
        GameHit {
            white: g.white_player_id.and_then(|id| names.get(&id).cloned()),
            black: g.black_player_id.and_then(|id| names.get(&id).cloned()),
            id: g.id,
            database_id: g.database_id,
            site: g.site,
            round: g.round,
            date: g.date,
            result: g.result,
            eco: g.eco,
            white_elo: g.white_elo,
            black_elo: g.black_elo,
            ply_count: g.ply_count,
        }
    }
}

/// Position search over the Zobrist `position_index`. Holds a connection handle
/// (cheap to clone — SeaORM wraps an `Arc`'d pool).
#[derive(Clone)]
pub struct PositionSearchService {
    db: DatabaseConnection,
}

impl PositionSearchService {
    pub fn new(db: DatabaseConnection) -> Self {
        Self { db }
    }

    /// Aggregated per-continuation statistics from the position described by
    /// `fen`, scoped to the caller. Sorted by occurrence count (descending),
    /// then SAN, for deterministic output. An unknown position yields an empty
    /// tree.
    pub async fn opening_tree(
        &self,
        user: &CurrentUser,
        fen: &str,
        filter: &PositionFilter,
    ) -> Result<Vec<MoveStat>, SearchError> {
        let zobrist = parse_zobrist(fen)?;
        let visible = self.visible_database_ids(user).await?;
        if visible.is_empty() {
            return Ok(Vec::new());
        }
        let cond = match self.filter_condition(filter).await? {
            None => return Ok(Vec::new()),
            Some(c) => c,
        };

        // Each indexed row at this position carries the move played from it; join
        // the owning game so its `result` arrives in the same query (no second
        // round-trip to fetch results by id).
        let rows: Vec<(String, i32, Option<String>)> = position_index::Entity::find()
            .filter(position_index::Column::Zobrist.eq(position_index::to_i64(zobrist)))
            .filter(position_index::Column::DatabaseId.is_in(visible))
            .join(JoinType::InnerJoin, position_index::Relation::Game.def())
            .filter(cond)
            .select_only()
            .column(position_index::Column::Move)
            .column(position_index::Column::GameId)
            .column(games::Column::Result)
            .into_tuple()
            .all(&self.db)
            .await?;
        if rows.is_empty() {
            return Ok(Vec::new());
        }

        // Count distinct games per continuation, not raw index rows: a game that
        // revisits this Zobrist (maneuvering/repetition — the Polyglot key ignores
        // clocks) must contribute its single result once, not once per occurrence.
        let mut games_by_san: HashMap<String, HashMap<i32, Option<String>>> = HashMap::new();
        for (san, game_id, result) in rows {
            games_by_san.entry(san).or_default().insert(game_id, result);
        }

        let mut tree: Vec<MoveStat> = games_by_san
            .into_iter()
            .map(|(san, games)| {
                let mut stat = MoveStat {
                    san,
                    count: games.len() as u64,
                    white: 0,
                    draws: 0,
                    black: 0,
                };
                for result in games.into_values() {
                    match result.as_deref() {
                        Some("1-0") => stat.white += 1,
                        Some("0-1") => stat.black += 1,
                        Some("1/2-1/2") => stat.draws += 1,
                        _ => {}
                    }
                }
                stat
            })
            .collect();
        tree.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.san.cmp(&b.san)));
        Ok(tree)
    }

    /// Games reaching the position described by `fen`, scoped to the caller and
    /// ordered oldest-first. `limit` caps the number of games returned.
    pub async fn games_with_position(
        &self,
        user: &CurrentUser,
        fen: &str,
        limit: Option<u64>,
        filter: &PositionFilter,
    ) -> Result<Vec<GameHit>, SearchError> {
        let zobrist = parse_zobrist(fen)?;
        let visible = self.visible_database_ids(user).await?;
        if visible.is_empty() {
            return Ok(Vec::new());
        }
        let cond = match self.filter_condition(filter).await? {
            None => return Ok(Vec::new()),
            Some(c) => c,
        };

        // A game may reach the same position more than once (repetition); keep the
        // distinct game-id set inside the database as a subquery instead of pulling
        // it into Rust only to bind it straight back as an `IN (?, ?, …)` list.
        let game_ids = position_index::Entity::find()
            .filter(position_index::Column::Zobrist.eq(position_index::to_i64(zobrist)))
            .filter(position_index::Column::DatabaseId.is_in(visible))
            .select_only()
            .column(position_index::Column::GameId)
            .distinct()
            .into_query();

        let mut query = games::Entity::find()
            .filter(games::Column::Id.in_subquery(game_ids))
            .filter(cond)
            .order_by_asc(games::Column::Id);
        if let Some(limit) = limit {
            query = query.limit(limit);
        }
        let rows = query.all(&self.db).await?;

        let names = crate::games::player_names(&self.db, &rows).await?;
        Ok(rows
            .into_iter()
            .map(|g| GameHit::from_model(g, &names))
            .collect())
    }

    /// The database ids visible to the caller (own ∪ global). Search filters the
    /// denormalized `position_index.database_id` against this set. Shared with the
    /// pre-chewed report layer ([`crate::search::report`]) so scope is computed once.
    pub(crate) async fn visible_database_ids(
        &self,
        user: &CurrentUser,
    ) -> Result<Vec<i32>, SearchError> {
        Ok(databases::Entity::find()
            .filter(scope(databases::Column::OwnerId, user))
            .select_only()
            .column(databases::Column::Id)
            .into_tuple()
            .all(&self.db)
            .await?)
    }

    /// The `games` filter condition for `filter`, or `None` if a player filter
    /// resolves to no matching players — the caller should short-circuit to an
    /// empty result without ever touching `position_index`/`games`.
    async fn filter_condition(
        &self,
        filter: &PositionFilter,
    ) -> Result<Option<Condition>, SearchError> {
        let mut cond = Condition::all();
        if let Some(name) = &filter.player {
            let ids = player_ids_matching(&self.db, name).await?;
            if ids.is_empty() {
                return Ok(None);
            }
            cond = cond.add(match filter.color {
                Some(Color::White) => games::Column::WhitePlayerId.is_in(ids).into_condition(),
                Some(Color::Black) => games::Column::BlackPlayerId.is_in(ids).into_condition(),
                None => Condition::any()
                    .add(games::Column::WhitePlayerId.is_in(ids.clone()))
                    .add(games::Column::BlackPlayerId.is_in(ids)),
            });
        }
        if let Some(from) = &filter.date_from {
            cond = cond.add(games::Column::Date.gte(from.clone()));
        }
        if let Some(to) = &filter.date_to {
            cond = cond.add(games::Column::Date.lte(to.clone()));
        }
        Ok(Some(cond))
    }
}

/// Hash `fen` to its Zobrist key, mapping a parse failure to [`SearchError`].
/// The key is variant-agnostic (ADR-0003); standard parsing covers normal FENs.
fn parse_zobrist(fen: &str) -> Result<u64, SearchError> {
    zobrist_of_fen(fen, CastlingMode::Standard).map_err(|e| SearchError::InvalidFen(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{connect, DbConfig};
    use crate::ingest::ingest_pgn;
    use crate::position::{replay, STARTPOS_FEN};
    use sea_orm::{ActiveModelTrait, Set};

    const STD: CastlingMode = CastlingMode::Standard;

    /// White wins with the scholar's mate (1. e4 e5 2. Bc4 Nc6 3. Qh5 Nf6 4. Qxf7#).
    const SCHOLARS_MATE: &str =
        "[White \"Spassky\"]\n[Black \"Fischer\"]\n[Result \"1-0\"]\n\n1. e4 e5 2. Bc4 Nc6 3. Qh5 Nf6 4. Qxf7# 1-0\n";
    /// A draw that opens 1. d4 d5.
    const QUEENS_DRAW: &str =
        "[White \"Carlsen\"]\n[Black \"Caruana\"]\n[Result \"1/2-1/2\"]\n\n1. d4 d5 2. c4 e6 1/2-1/2\n";
    /// Black wins, also opening 1. e4 e5 (shares the start continuation with the mate).
    const BLACK_WIN_E4: &str =
        "[White \"Nepo\"]\n[Black \"Ding\"]\n[Result \"0-1\"]\n\n1. e4 e5 2. Nf3 Nc6 0-1\n";
    /// A knight shuffle that returns to the start position before playing on: the
    /// start (and its `Nf3` continuation) is indexed twice for this single game.
    const KNIGHT_SHUFFLE: &str =
        "[White \"Loop\"]\n[Black \"Back\"]\n[Result \"1-0\"]\n\n1. Nf3 Nf6 2. Ng1 Ng8 3. Nf3 Nf6 1-0\n";

    fn user(id: &str) -> CurrentUser {
        CurrentUser {
            id: id.to_string(),
            is_admin: false,
        }
    }

    async fn db_for(owner: &str) -> (DatabaseConnection, i32) {
        let conn = connect(&DbConfig::in_memory()).await.unwrap();
        let db = databases::ActiveModel {
            owner_id: Set(Some(owner.to_string())),
            name: Set("games".to_string()),
            kind: Set("own".to_string()),
            ..Default::default()
        }
        .insert(&conn)
        .await
        .unwrap();
        (conn, db.id)
    }

    #[tokio::test]
    async fn start_position_aggregates_move_distribution() {
        let (conn, db_id) = db_for("alice").await;
        for pgn in [SCHOLARS_MATE, QUEENS_DRAW, BLACK_WIN_E4] {
            ingest_pgn(&conn, db_id, pgn).await.unwrap();
        }
        let svc = PositionSearchService::new(conn);

        let tree = svc
            .opening_tree(&user("alice"), STARTPOS_FEN, &PositionFilter::default())
            .await
            .unwrap();
        // Two distinct first moves: e4 (twice) and d4 (once); e4 ranks first.
        assert_eq!(tree.len(), 2);
        assert_eq!(tree[0].san, "e4");
        assert_eq!(tree[0].count, 2);
        assert_eq!(tree[0].white, 1); // the mate
        assert_eq!(tree[0].black, 1); // the e4 black win
        assert_eq!(tree[0].draws, 0);
        assert_eq!(tree[1].san, "d4");
        assert_eq!(tree[1].count, 1);
        assert_eq!(tree[1].draws, 1);
    }

    #[tokio::test]
    async fn midgame_position_splits_continuations() {
        let (conn, db_id) = db_for("alice").await;
        ingest_pgn(&conn, db_id, SCHOLARS_MATE).await.unwrap();
        ingest_pgn(&conn, db_id, BLACK_WIN_E4).await.unwrap();
        let svc = PositionSearchService::new(conn);

        // Position after 1. e4 e5: the two games diverge (Bc4 vs Nf3).
        let after = replay(STARTPOS_FEN, &["e4", "e5"], STD).unwrap();
        let fen = &after.last().unwrap().fen;
        let tree = svc
            .opening_tree(&user("alice"), fen, &PositionFilter::default())
            .await
            .unwrap();
        let sans: Vec<&str> = tree.iter().map(|m| m.san.as_str()).collect();
        assert_eq!(sans, vec!["Bc4", "Nf3"]);
        assert!(tree.iter().all(|m| m.count == 1));
    }

    #[tokio::test]
    async fn revisited_position_counts_each_game_once() {
        let (conn, db_id) = db_for("alice").await;
        // This game reaches the start position twice (knight shuffle) and plays
        // Nf3 from it both times; it must count as one game, not two.
        ingest_pgn(&conn, db_id, KNIGHT_SHUFFLE).await.unwrap();
        let svc = PositionSearchService::new(conn);

        let tree = svc
            .opening_tree(&user("alice"), STARTPOS_FEN, &PositionFilter::default())
            .await
            .unwrap();
        assert_eq!(tree.len(), 1);
        assert_eq!(tree[0].san, "Nf3");
        assert_eq!(tree[0].count, 1); // distinct games, not the 2 indexed rows
        assert_eq!(tree[0].white, 1); // the single 1-0 result, counted once
    }

    #[tokio::test]
    async fn games_with_position_returns_only_matching_games() {
        let (conn, db_id) = db_for("alice").await;
        ingest_pgn(&conn, db_id, SCHOLARS_MATE).await.unwrap();
        ingest_pgn(&conn, db_id, QUEENS_DRAW).await.unwrap();
        let svc = PositionSearchService::new(conn);

        // After 1. e4 e5 only the scholar's mate game reaches it.
        let after = replay(STARTPOS_FEN, &["e4", "e5"], STD).unwrap();
        let hits = svc
            .games_with_position(
                &user("alice"),
                &after.last().unwrap().fen,
                None,
                &PositionFilter::default(),
            )
            .await
            .unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].white.as_deref(), Some("Spassky"));
        assert_eq!(hits[0].black.as_deref(), Some("Fischer"));
        assert_eq!(hits[0].result.as_deref(), Some("1-0"));
    }

    #[tokio::test]
    async fn search_scope_excludes_other_users_databases() {
        let (conn, alice_db) = db_for("alice").await;
        // A second database owned by bob in the same connection.
        let bob_db = databases::ActiveModel {
            owner_id: Set(Some("bob".to_string())),
            name: Set("bob".to_string()),
            kind: Set("own".to_string()),
            ..Default::default()
        }
        .insert(&conn)
        .await
        .unwrap()
        .id;
        ingest_pgn(&conn, alice_db, SCHOLARS_MATE).await.unwrap();
        ingest_pgn(&conn, bob_db, QUEENS_DRAW).await.unwrap();
        let svc = PositionSearchService::new(conn);

        // Bob's draw opens 1. d4 d5; alice must not see it in her start-position tree.
        let tree = svc
            .opening_tree(&user("alice"), STARTPOS_FEN, &PositionFilter::default())
            .await
            .unwrap();
        assert_eq!(tree.len(), 1);
        assert_eq!(tree[0].san, "e4");

        // Bob, conversely, only sees his own opening.
        let bob_tree = svc
            .opening_tree(&user("bob"), STARTPOS_FEN, &PositionFilter::default())
            .await
            .unwrap();
        assert_eq!(bob_tree.len(), 1);
        assert_eq!(bob_tree[0].san, "d4");
    }

    #[tokio::test]
    async fn global_database_is_visible_to_every_user() {
        let conn = connect(&DbConfig::in_memory()).await.unwrap();
        let global = databases::ActiveModel {
            owner_id: Set(None),
            name: Set("masters".to_string()),
            kind: Set("master".to_string()),
            ..Default::default()
        }
        .insert(&conn)
        .await
        .unwrap()
        .id;
        ingest_pgn(&conn, global, SCHOLARS_MATE).await.unwrap();
        let svc = PositionSearchService::new(conn);

        let tree = svc
            .opening_tree(&user("anyone"), STARTPOS_FEN, &PositionFilter::default())
            .await
            .unwrap();
        assert_eq!(tree.len(), 1);
        assert_eq!(tree[0].san, "e4");
    }

    #[tokio::test]
    async fn unknown_position_yields_empty_results() {
        let (conn, db_id) = db_for("alice").await;
        ingest_pgn(&conn, db_id, SCHOLARS_MATE).await.unwrap();
        let svc = PositionSearchService::new(conn);

        // A legal but un-indexed position (after 1. h4).
        let after = replay(STARTPOS_FEN, &["h4"], STD).unwrap();
        let fen = &after.last().unwrap().fen;
        assert!(svc
            .opening_tree(&user("alice"), fen, &PositionFilter::default())
            .await
            .unwrap()
            .is_empty());
        assert!(svc
            .games_with_position(&user("alice"), fen, None, &PositionFilter::default())
            .await
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn invalid_fen_is_rejected() {
        let (conn, _) = db_for("alice").await;
        let svc = PositionSearchService::new(conn);
        let err = svc
            .opening_tree(&user("alice"), "not a fen", &PositionFilter::default())
            .await;
        assert!(matches!(err, Err(SearchError::InvalidFen(_))));
    }

    /// `escape_like` is also exercised indirectly by header search's `LIKE`
    /// wildcard test (`headers_tests.rs`); this covers it directly since it now
    /// lives here (issue #172, moved from `search::headers`).
    #[test]
    fn escape_like_neutralizes_wildcards() {
        assert_eq!(escape_like("a%b_c"), "a\\%b\\_c");
        assert_eq!(escape_like("100%"), "100\\%");
        // Backslash is escaped first so it can't form a spurious escape sequence.
        assert_eq!(escape_like("a\\_b"), "a\\\\\\_b");
        assert_eq!(escape_like("plain"), "plain");
    }

    // --- PositionFilter (issue #172) -----------------------------------

    /// Carlsen playing White, opening 1. e4 e5 2. Nf3 Nc6, dated mid-2021.
    const CARLSEN_WHITE: &str = "[White \"Carlsen\"]\n[Black \"Nepo\"]\n[Result \"1-0\"]\n\
        [Date \"2021.06.01\"]\n\n1. e4 e5 2. Nf3 Nc6 1-0\n";
    /// Carlsen playing Black, same 1. e4 e5 stem, dated mid-2022.
    const CARLSEN_BLACK: &str = "[White \"Nepo\"]\n[Black \"Carlsen\"]\n[Result \"0-1\"]\n\
        [Date \"2022.06.01\"]\n\n1. e4 e5 2. Bc4 Bc5 0-1\n";
    /// Neither side is Carlsen, same stem, dated 2020 (before both games above).
    const OTHERS_GAME: &str = "[White \"Nepo\"]\n[Black \"Ding\"]\n[Result \"1/2-1/2\"]\n\
        [Date \"2020.01.01\"]\n\n1. e4 e5 2. Nc3 Nc6 1/2-1/2\n";

    /// The FEN after 1. e4 e5, where the three fixtures above diverge.
    fn after_e4e5() -> String {
        replay(STARTPOS_FEN, &["e4", "e5"], STD)
            .unwrap()
            .last()
            .unwrap()
            .fen
            .clone()
    }

    #[tokio::test]
    async fn player_filter_matches_either_side() {
        let (conn, db_id) = db_for("alice").await;
        for pgn in [CARLSEN_WHITE, CARLSEN_BLACK, OTHERS_GAME] {
            ingest_pgn(&conn, db_id, pgn).await.unwrap();
        }
        let svc = PositionSearchService::new(conn);
        let fen = after_e4e5();

        let filter = PositionFilter {
            player: Some("Carlsen".to_string()),
            ..Default::default()
        };
        let tree = svc
            .opening_tree(&user("alice"), &fen, &filter)
            .await
            .unwrap();
        // Both Carlsen games (White's Nf3, Black's Bc4) survive; the third
        // game's Nc3 (neither side Carlsen) does not.
        let sans: Vec<&str> = tree.iter().map(|m| m.san.as_str()).collect();
        assert_eq!(sans, vec!["Bc4", "Nf3"]);

        let hits = svc
            .games_with_position(&user("alice"), &fen, None, &filter)
            .await
            .unwrap();
        assert_eq!(hits.len(), 2);
    }

    #[tokio::test]
    async fn player_plus_color_restricts_to_one_side() {
        let (conn, db_id) = db_for("alice").await;
        for pgn in [CARLSEN_WHITE, CARLSEN_BLACK, OTHERS_GAME] {
            ingest_pgn(&conn, db_id, pgn).await.unwrap();
        }
        let svc = PositionSearchService::new(conn);
        let fen = after_e4e5();

        let filter = PositionFilter {
            player: Some("Carlsen".to_string()),
            color: Some(Color::White),
            ..Default::default()
        };
        let tree = svc
            .opening_tree(&user("alice"), &fen, &filter)
            .await
            .unwrap();
        assert_eq!(tree.len(), 1);
        assert_eq!(tree[0].san, "Nf3");

        let hits = svc
            .games_with_position(&user("alice"), &fen, None, &filter)
            .await
            .unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].white.as_deref(), Some("Carlsen"));
    }

    #[tokio::test]
    async fn date_range_excludes_games_outside_it() {
        let (conn, db_id) = db_for("alice").await;
        for pgn in [CARLSEN_WHITE, CARLSEN_BLACK, OTHERS_GAME] {
            ingest_pgn(&conn, db_id, pgn).await.unwrap();
        }
        let svc = PositionSearchService::new(conn);
        let fen = after_e4e5();

        // Keeps only the 2021 game (Carlsen-White's Nf3); excludes the 2020 and
        // 2022 games.
        let filter = PositionFilter {
            date_from: Some("2021.01.01".to_string()),
            date_to: Some("2021.12.31".to_string()),
            ..Default::default()
        };
        let tree = svc
            .opening_tree(&user("alice"), &fen, &filter)
            .await
            .unwrap();
        assert_eq!(tree.len(), 1);
        assert_eq!(tree[0].san, "Nf3");
    }

    #[tokio::test]
    async fn unmatched_player_yields_empty_results_not_an_error() {
        let (conn, db_id) = db_for("alice").await;
        ingest_pgn(&conn, db_id, CARLSEN_WHITE).await.unwrap();
        let svc = PositionSearchService::new(conn);

        let filter = PositionFilter {
            player: Some("Nobody".to_string()),
            ..Default::default()
        };
        assert!(svc
            .opening_tree(&user("alice"), STARTPOS_FEN, &filter)
            .await
            .unwrap()
            .is_empty());
        assert!(svc
            .games_with_position(&user("alice"), STARTPOS_FEN, None, &filter)
            .await
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn color_without_player_is_a_no_op() {
        // Mirrors header search's semantics (`search::headers`): a color with no
        // player filter is meaningless (every game already has both a White and
        // Black side), so it must not narrow the result on its own.
        let (conn, db_id) = db_for("alice").await;
        for pgn in [CARLSEN_WHITE, CARLSEN_BLACK, OTHERS_GAME] {
            ingest_pgn(&conn, db_id, pgn).await.unwrap();
        }
        let svc = PositionSearchService::new(conn);
        let fen = after_e4e5();

        let filter = PositionFilter {
            color: Some(Color::White),
            ..Default::default()
        };
        let tree = svc
            .opening_tree(&user("alice"), &fen, &filter)
            .await
            .unwrap();
        // All three games' continuations still present, unchanged from the
        // filter-less tree.
        let sans: Vec<&str> = tree.iter().map(|m| m.san.as_str()).collect();
        assert_eq!(sans, vec!["Bc4", "Nc3", "Nf3"]);
    }
}
