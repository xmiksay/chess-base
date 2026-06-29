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

use std::collections::{HashMap, HashSet};

use sea_orm::{
    ColumnTrait, DatabaseConnection, DbErr, EntityTrait, QueryFilter, QueryOrder, QuerySelect,
};
use serde::Serialize;

use crate::db::entities::{databases, games, position_index};
use crate::position::{zobrist_of_fen, CastlingMode};
use crate::server::identity::{scope, CurrentUser};

/// Why a position search failed. Transport-agnostic — the HTTP / MCP layer maps
/// each variant onto its own status / error envelope.
#[derive(Debug, thiserror::Error)]
pub enum SearchError {
    /// The supplied FEN could not be parsed into a legal position.
    #[error("invalid FEN: {0}")]
    InvalidFen(String),
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
    ) -> Result<Vec<MoveStat>, SearchError> {
        let zobrist = parse_zobrist(fen)?;
        let visible = self.visible_database_ids(user).await?;
        if visible.is_empty() {
            return Ok(Vec::new());
        }

        // Each indexed row at this position carries the move played from it; join
        // the owning game's result in-process to break the count down by outcome.
        let rows: Vec<(String, i32)> = position_index::Entity::find()
            .filter(position_index::Column::Zobrist.eq(position_index::to_i64(zobrist)))
            .filter(position_index::Column::DatabaseId.is_in(visible))
            .select_only()
            .column(position_index::Column::Move)
            .column(position_index::Column::GameId)
            .into_tuple()
            .all(&self.db)
            .await?;
        if rows.is_empty() {
            return Ok(Vec::new());
        }

        let results = self.results_for(rows.iter().map(|(_, g)| *g)).await?;

        // Count distinct games per continuation, not raw index rows: a game that
        // revisits this Zobrist (maneuvering/repetition — the Polyglot key ignores
        // clocks) must contribute its single result once, not once per occurrence.
        let mut games_by_san: HashMap<String, HashSet<i32>> = HashMap::new();
        for (san, game_id) in rows {
            games_by_san.entry(san).or_default().insert(game_id);
        }

        let mut tree: Vec<MoveStat> = games_by_san
            .into_iter()
            .map(|(san, game_ids)| {
                let mut stat = MoveStat {
                    san,
                    count: game_ids.len() as u64,
                    white: 0,
                    draws: 0,
                    black: 0,
                };
                for game_id in game_ids {
                    match results.get(&game_id).and_then(Option::as_deref) {
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
    ) -> Result<Vec<GameHit>, SearchError> {
        let zobrist = parse_zobrist(fen)?;
        let visible = self.visible_database_ids(user).await?;
        if visible.is_empty() {
            return Ok(Vec::new());
        }

        // A game may reach the same position more than once (repetition), so take
        // distinct game ids before loading the games themselves.
        let game_ids: Vec<i32> = position_index::Entity::find()
            .filter(position_index::Column::Zobrist.eq(position_index::to_i64(zobrist)))
            .filter(position_index::Column::DatabaseId.is_in(visible))
            .select_only()
            .column(position_index::Column::GameId)
            .distinct()
            .into_tuple()
            .all(&self.db)
            .await?;
        if game_ids.is_empty() {
            return Ok(Vec::new());
        }

        let mut query = games::Entity::find()
            .filter(games::Column::Id.is_in(game_ids))
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

    /// `game_id -> result` for the given games, loaded in one query.
    async fn results_for(
        &self,
        game_ids: impl Iterator<Item = i32>,
    ) -> Result<HashMap<i32, Option<String>>, SearchError> {
        let ids: HashSet<i32> = game_ids.collect();
        Ok(games::Entity::find()
            .filter(games::Column::Id.is_in(ids))
            .select_only()
            .column(games::Column::Id)
            .column(games::Column::Result)
            .into_tuple::<(i32, Option<String>)>()
            .all(&self.db)
            .await?
            .into_iter()
            .collect())
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
            .opening_tree(&user("alice"), STARTPOS_FEN)
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
        let tree = svc.opening_tree(&user("alice"), fen).await.unwrap();
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
            .opening_tree(&user("alice"), STARTPOS_FEN)
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
            .games_with_position(&user("alice"), &after.last().unwrap().fen, None)
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
            .opening_tree(&user("alice"), STARTPOS_FEN)
            .await
            .unwrap();
        assert_eq!(tree.len(), 1);
        assert_eq!(tree[0].san, "e4");

        // Bob, conversely, only sees his own opening.
        let bob_tree = svc.opening_tree(&user("bob"), STARTPOS_FEN).await.unwrap();
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
            .opening_tree(&user("anyone"), STARTPOS_FEN)
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
            .opening_tree(&user("alice"), fen)
            .await
            .unwrap()
            .is_empty());
        assert!(svc
            .games_with_position(&user("alice"), fen, None)
            .await
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn invalid_fen_is_rejected() {
        let (conn, _) = db_for("alice").await;
        let svc = PositionSearchService::new(conn);
        let err = svc.opening_tree(&user("alice"), "not a fen").await;
        assert!(matches!(err, Err(SearchError::InvalidFen(_))));
    }
}
