//! Pre-chewed DB query layer (issue #28): turns the raw Zobrist position index
//! into *synthesized* answers — ECO classification, per-move win/draw/loss with
//! frequency, transpositions (the same position reached via different move
//! orders) and reference games — so an MCP/LLM caller consumes conclusions, not
//! raw rows (ADR-0009: the model synthesizes, it never computes).
//!
//! Built on top of [`PositionSearchService`] (issue #7): move aggregation and
//! game lookup are reused verbatim; this layer only adds ECO classification,
//! the derived frequency/score figures, and transposition reconstruction. Scope
//! follows the same ownership rule (own ∪ global), so every method takes a
//! [`CurrentUser`].

use std::collections::HashMap;

use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder, QuerySelect};
use serde::{Deserialize, Serialize};

use crate::db::entities::position_index;
use crate::openings::opening_of_zobrist;
use crate::position::{zobrist_of_fen, CastlingMode};
use crate::search::position::{GameHit, MoveStat, PositionSearchService, SearchError};
use crate::server::identity::CurrentUser;

/// ECO classification (code + name) for a position.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EcoInfo {
    pub eco: String,
    pub name: String,
}

/// A continuation with its outcomes (reused from #7) plus the two figures the
/// raw index can't give: `frequency` (share of games that chose this move,
/// `0..=1`) and `score` (White's performance, `0..=1`). `white`/`draws`/`black`
/// are the win/draw/loss counts; games with an unknown result (`*`) count toward
/// `count` only and are excluded from `score`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MoveReport {
    pub san: String,
    pub count: u64,
    pub white: u64,
    pub draws: u64,
    pub black: u64,
    pub frequency: f64,
    pub score: f64,
}

impl MoveReport {
    /// Layer frequency/score onto an aggregated [`MoveStat`]. `total` is the sum
    /// of all continuation counts at the position (the frequency denominator).
    fn from_stat(stat: MoveStat, total: u64) -> Self {
        let decided = stat.white + stat.draws + stat.black;
        let score = if decided == 0 {
            0.0
        } else {
            (stat.white as f64 + stat.draws as f64 / 2.0) / decided as f64
        };
        let frequency = if total == 0 {
            0.0
        } else {
            stat.count as f64 / total as f64
        };
        Self {
            san: stat.san,
            count: stat.count,
            white: stat.white,
            draws: stat.draws,
            black: stat.black,
            frequency,
            score,
        }
    }
}

/// One move order that reaches the queried position: the SAN line from the start
/// and how many games took it. Two entries on the same position are
/// transpositions of one another.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Transposition {
    pub line: Vec<String>,
    pub ply: u32,
    pub games: u64,
}

/// The synthesized report for one position.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PositionReport {
    pub fen: String,
    /// Zobrist key as zero-padded hex (avoids 64-bit precision loss in JSON).
    pub zobrist: String,
    pub eco: Option<EcoInfo>,
    /// Total continuations counted at the position (the frequency denominator).
    pub total: u64,
    pub moves: Vec<MoveReport>,
    pub transpositions: Vec<Transposition>,
}

/// Pre-chewed DB query layer. Wraps [`PositionSearchService`] (reused for move
/// stats and game lookup) and adds ECO / transposition synthesis. Holds a
/// connection handle (cheap to clone — SeaORM wraps an `Arc`'d pool).
#[derive(Clone)]
pub struct PositionReportService {
    search: PositionSearchService,
    db: DatabaseConnection,
}

impl PositionReportService {
    pub fn new(db: DatabaseConnection) -> Self {
        Self {
            search: PositionSearchService::new(db.clone()),
            db,
        }
    }

    /// The full synthesized report for `fen`: ECO + per-move win/draw/loss with
    /// frequency/score (from #7) + transpositions, all scoped to the caller. An
    /// un-played position still returns its ECO (the dataset is static); the move
    /// and transposition lists are simply empty.
    pub async fn position_report(
        &self,
        user: &CurrentUser,
        fen: &str,
    ) -> Result<PositionReport, SearchError> {
        let zobrist = zobrist_of_fen(fen, CastlingMode::Standard)
            .map_err(|e| SearchError::InvalidFen(e.to_string()))?;

        let stats = self.search.opening_tree(user, fen).await?;
        let total: u64 = stats.iter().map(|m| m.count).sum();
        let moves = stats
            .into_iter()
            .map(|s| MoveReport::from_stat(s, total))
            .collect();

        let eco = opening_of_zobrist(zobrist).map(|o| EcoInfo {
            eco: o.eco.to_string(),
            name: o.name.to_string(),
        });

        let visible = self.search.visible_database_ids(user).await?;
        let transpositions = if visible.is_empty() {
            Vec::new()
        } else {
            self.transpositions(zobrist, visible).await?
        };

        Ok(PositionReport {
            fen: fen.to_string(),
            zobrist: format!("{zobrist:016x}"),
            eco,
            total,
            moves,
            transpositions,
        })
    }

    /// Batch [`Self::position_report`] over several FENs (one round of queries
    /// per FEN). A single invalid FEN fails the whole batch.
    pub async fn position_reports(
        &self,
        user: &CurrentUser,
        fens: &[impl AsRef<str>],
    ) -> Result<Vec<PositionReport>, SearchError> {
        let mut reports = Vec::with_capacity(fens.len());
        for fen in fens {
            reports.push(self.position_report(user, fen.as_ref()).await?);
        }
        Ok(reports)
    }

    /// Reference / typical games reaching the position, scoped to the caller and
    /// capped by `limit`. A thin reuse of
    /// [`PositionSearchService::games_with_position`].
    pub async fn references(
        &self,
        user: &CurrentUser,
        fen: &str,
        limit: Option<u64>,
    ) -> Result<Vec<GameHit>, SearchError> {
        self.search.games_with_position(user, fen, limit).await
    }

    /// Reconstruct the distinct move orders that reach the position. For every
    /// scoped game that hits the Zobrist, take the first (lowest-ply) arrival and
    /// replay its indexed moves up to that ply; identical SAN lines collapse into
    /// one transposition. Sorted by game count (desc) then line.
    async fn transpositions(
        &self,
        zobrist: u64,
        visible: Vec<i32>,
    ) -> Result<Vec<Transposition>, SearchError> {
        let occurrences: Vec<(i32, i32)> = position_index::Entity::find()
            .filter(position_index::Column::Zobrist.eq(position_index::to_i64(zobrist)))
            .filter(position_index::Column::DatabaseId.is_in(visible))
            .select_only()
            .column(position_index::Column::GameId)
            .column(position_index::Column::Ply)
            .into_tuple()
            .all(&self.db)
            .await?;
        if occurrences.is_empty() {
            return Ok(Vec::new());
        }

        // First arrival ply per game: the move order by which it first reached here.
        let mut arrival: HashMap<i32, i32> = HashMap::new();
        for (game_id, ply) in occurrences {
            arrival
                .entry(game_id)
                .and_modify(|p| *p = (*p).min(ply))
                .or_insert(ply);
        }

        // Load every indexed move of those games; the line reaching the position
        // at `arrival[g]` is each move played at a lower ply.
        let game_ids: Vec<i32> = arrival.keys().copied().collect();
        let rows: Vec<(i32, i32, String)> = position_index::Entity::find()
            .filter(position_index::Column::GameId.is_in(game_ids))
            .select_only()
            .column(position_index::Column::GameId)
            .column(position_index::Column::Ply)
            .column(position_index::Column::Move)
            .order_by_asc(position_index::Column::Ply)
            .into_tuple()
            .all(&self.db)
            .await?;
        let mut moves_by_game: HashMap<i32, Vec<(i32, String)>> = HashMap::new();
        for (game_id, ply, san) in rows {
            moves_by_game.entry(game_id).or_default().push((ply, san));
        }

        let mut lines: HashMap<Vec<String>, u64> = HashMap::new();
        for (game_id, target) in arrival {
            let line: Vec<String> = moves_by_game
                .get(&game_id)
                .map(|moves| {
                    moves
                        .iter()
                        .filter(|(ply, _)| *ply < target)
                        .map(|(_, san)| san.clone())
                        .collect()
                })
                .unwrap_or_default();
            *lines.entry(line).or_insert(0) += 1;
        }

        let mut transpositions: Vec<Transposition> = lines
            .into_iter()
            .map(|(line, games)| Transposition {
                ply: line.len() as u32,
                line,
                games,
            })
            .collect();
        transpositions.sort_by(|a, b| b.games.cmp(&a.games).then_with(|| a.line.cmp(&b.line)));
        Ok(transpositions)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::entities::databases;
    use crate::db::{connect, DbConfig};
    use crate::ingest::ingest_pgn;
    use crate::position::{replay, STARTPOS_FEN};
    use sea_orm::{ActiveModelTrait, Set};

    const STD: CastlingMode = CastlingMode::Standard;

    // 1. e4 c5 2. Nf3 d6 — White wins; a Sicilian.
    const SICILIAN_WHITE: &str = "[Result \"1-0\"]\n\n1. e4 c5 2. Nf3 d6 3. d4 cxd4 1-0\n";
    // 1. e4 c5 2. Nf3 Nc6 — Black wins; shares the 1. e4 c5 2. Nf3 stem.
    const SICILIAN_BLACK: &str = "[Result \"0-1\"]\n\n1. e4 c5 2. Nf3 Nc6 3. d4 cxd4 0-1\n";

    // Two games reaching the SAME position after the 4th half-move via different
    // move orders, then both playing Nc3 so that position is actually indexed.
    const ORDER_A: &str = "[Result \"1-0\"]\n\n1. d4 Nf6 2. c4 g6 3. Nc3 d5 1-0\n";
    const ORDER_B: &str = "[Result \"0-1\"]\n\n1. c4 g6 2. d4 Nf6 3. Nc3 d5 0-1\n";

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

    fn fen_after(sans: &[&str]) -> String {
        replay(STARTPOS_FEN, sans, STD)
            .unwrap()
            .last()
            .unwrap()
            .fen
            .clone()
    }

    #[tokio::test]
    async fn report_classifies_eco_and_move_frequency() {
        let (conn, db_id) = db_for("alice").await;
        for pgn in [SICILIAN_WHITE, SICILIAN_BLACK] {
            ingest_pgn(&conn, db_id, pgn).await.unwrap();
        }
        let svc = PositionReportService::new(conn);

        // After 1. e4 c5 both games continue with Nf3, so it is the lone move.
        let report = svc
            .position_report(&user("alice"), &fen_after(&["e4", "c5"]))
            .await
            .unwrap();
        let eco = report.eco.expect("Sicilian is a known opening");
        assert_eq!(eco.eco, "B20");
        assert!(eco.name.starts_with("Sicilian"), "got {}", eco.name);
        assert_eq!(report.total, 2);
        assert_eq!(report.moves.len(), 1);
        assert_eq!(report.moves[0].san, "Nf3");
        assert_eq!(report.moves[0].count, 2);
        assert_eq!(report.moves[0].frequency, 1.0);
    }

    #[tokio::test]
    async fn report_splits_frequency_and_score_across_continuations() {
        let (conn, db_id) = db_for("alice").await;
        for pgn in [SICILIAN_WHITE, SICILIAN_BLACK] {
            ingest_pgn(&conn, db_id, pgn).await.unwrap();
        }
        let svc = PositionReportService::new(conn);

        // After 1. e4 c5 2. Nf3 the games diverge: d6 (White won) vs Nc6 (Black won).
        let report = svc
            .position_report(&user("alice"), &fen_after(&["e4", "c5", "Nf3"]))
            .await
            .unwrap();
        assert_eq!(report.total, 2);
        let d6 = report.moves.iter().find(|m| m.san == "d6").unwrap();
        assert_eq!(d6.frequency, 0.5);
        assert_eq!(d6.white, 1);
        assert_eq!(d6.score, 1.0); // White scored 1/1
        let nc6 = report.moves.iter().find(|m| m.san == "Nc6").unwrap();
        assert_eq!(nc6.frequency, 0.5);
        assert_eq!(nc6.black, 1);
        assert_eq!(nc6.score, 0.0); // White scored 0/1
    }

    #[tokio::test]
    async fn report_lists_transpositions_reaching_the_position() {
        let (conn, db_id) = db_for("alice").await;
        ingest_pgn(&conn, db_id, ORDER_A).await.unwrap();
        ingest_pgn(&conn, db_id, ORDER_B).await.unwrap();
        let svc = PositionReportService::new(conn);

        // Both games reach this position; each by a distinct move order.
        let report = svc
            .position_report(&user("alice"), &fen_after(&["d4", "Nf6", "c4", "g6"]))
            .await
            .unwrap();

        assert_eq!(report.transpositions.len(), 2);
        for t in &report.transpositions {
            assert_eq!(t.games, 1);
            assert_eq!(t.ply, 4);
        }
        let lines: Vec<&[String]> = report.transpositions.iter().map(|t| &t.line[..]).collect();
        assert!(lines.contains(&&["d4", "Nf6", "c4", "g6"].map(String::from)[..]));
        assert!(lines.contains(&&["c4", "g6", "d4", "Nf6"].map(String::from)[..]));

        // The shared continuation (Nc3) is aggregated across both move orders.
        let nc3 = report.moves.iter().find(|m| m.san == "Nc3").unwrap();
        assert_eq!(nc3.count, 2);
        assert_eq!(nc3.score, 0.5); // one White win, one Black win
    }

    #[tokio::test]
    async fn references_return_scoped_games() {
        let (conn, db_id) = db_for("alice").await;
        ingest_pgn(&conn, db_id, SICILIAN_WHITE).await.unwrap();
        ingest_pgn(&conn, db_id, SICILIAN_BLACK).await.unwrap();
        let svc = PositionReportService::new(conn);

        // After 1. e4 c5 2. Nf3 d6 only the White-win game is a reference.
        let games = svc
            .references(&user("alice"), &fen_after(&["e4", "c5", "Nf3", "d6"]), None)
            .await
            .unwrap();
        assert_eq!(games.len(), 1);
        assert_eq!(games[0].result.as_deref(), Some("1-0"));
    }

    #[tokio::test]
    async fn batch_reports_one_per_fen() {
        let (conn, db_id) = db_for("alice").await;
        ingest_pgn(&conn, db_id, SICILIAN_WHITE).await.unwrap();
        let svc = PositionReportService::new(conn);

        let fens = [fen_after(&["e4", "c5"]), fen_after(&["e4", "c5", "Nf3"])];
        let reports = svc.position_reports(&user("alice"), &fens).await.unwrap();
        assert_eq!(reports.len(), 2);
        assert_eq!(reports[0].fen, fens[0]);
        assert_eq!(reports[1].fen, fens[1]);
    }

    #[tokio::test]
    async fn unknown_position_reports_no_moves() {
        let (conn, db_id) = db_for("alice").await;
        ingest_pgn(&conn, db_id, SICILIAN_WHITE).await.unwrap();
        let svc = PositionReportService::new(conn);

        // A legal but un-indexed position (after 1. h4).
        let report = svc
            .position_report(&user("alice"), &fen_after(&["h4"]))
            .await
            .unwrap();
        assert_eq!(report.total, 0);
        assert!(report.moves.is_empty());
        assert!(report.transpositions.is_empty());
    }

    #[tokio::test]
    async fn invalid_fen_is_rejected() {
        let (conn, _) = db_for("alice").await;
        let svc = PositionReportService::new(conn);
        let err = svc.position_report(&user("alice"), "not a fen").await;
        assert!(matches!(err, Err(SearchError::InvalidFen(_))));
    }
}
