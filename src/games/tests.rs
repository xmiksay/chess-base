//! Service-level tests over an in-memory SQLite DB: offset pagination (page +
//! total + limit clamping), sorting (default newest-first, by date/result/eco/
//! added), the visibility scope (own ∪ global, never another user's), single-game
//! fetch with PGN, and resolved player names.

use super::*;
use crate::db::{connect, DbConfig};
use crate::ingest::ingest_pgn;
use sea_orm::{ActiveModelTrait, Set};

const SCHOLARS_MATE: &str =
    "[White \"Spassky\"]\n[Black \"Fischer\"]\n[Result \"1-0\"]\n\n1. e4 e5 2. Bc4 Nc6 3. Qh5 Nf6 4. Qxf7# 1-0\n";
const QUEENS_DRAW: &str =
    "[White \"Carlsen\"]\n[Black \"Caruana\"]\n[Result \"1/2-1/2\"]\n\n1. d4 d5 2. c4 e6 1/2-1/2\n";

fn user(id: &str) -> CurrentUser {
    CurrentUser {
        id: id.to_string(),
        is_admin: false,
    }
}

/// A list request for `database_id` using the service defaults (date,
/// newest-first, first page, default limit).
fn params(database_id: i32) -> GameListParams {
    GameListParams {
        database_id,
        page: 0,
        limit: DEFAULT_LIMIT,
        sort: GameSort::default(),
        dir: SortDir::default(),
    }
}

/// Create a database for `owner` (None ⇒ global) and return (conn, database id).
async fn db_for(owner: Option<&str>) -> (DatabaseConnection, i32) {
    let conn = connect(&DbConfig::in_memory()).await.unwrap();
    let id = make_db(&conn, owner).await;
    (conn, id)
}

async fn make_db(conn: &DatabaseConnection, owner: Option<&str>) -> i32 {
    databases::ActiveModel {
        owner_id: Set(owner.map(str::to_string)),
        name: Set("games".to_string()),
        kind: Set(if owner.is_some() { "own" } else { "master" }.to_string()),
        ..Default::default()
    }
    .insert(conn)
    .await
    .unwrap()
    .id
}

#[tokio::test]
async fn list_returns_games_newest_first_with_total_and_names() {
    let (conn, db_id) = db_for(Some("alice")).await;
    ingest_pgn(&conn, db_id, SCHOLARS_MATE).await.unwrap();
    ingest_pgn(&conn, db_id, QUEENS_DRAW).await.unwrap();
    let svc = GameService::new(conn);

    let page = svc.list(&user("alice"), &params(db_id)).await.unwrap();
    assert_eq!(page.games.len(), 2);
    assert_eq!(page.total, 2);
    assert_eq!(page.page, 0);
    // Default sort is date-desc; these games share no Date tag, so the `id`
    // tiebreaker (also desc) puts the most-recently-added game first.
    assert_eq!(page.games[0].white.as_deref(), Some("Carlsen"));
    assert_eq!(page.games[1].white.as_deref(), Some("Spassky"));
    assert!(page.games[0].id > page.games[1].id);
}

#[tokio::test]
async fn list_paginates_by_offset_and_reports_total() {
    let (conn, db_id) = db_for(Some("alice")).await;
    for _ in 0..5 {
        ingest_pgn(&conn, db_id, SCHOLARS_MATE).await.unwrap();
    }
    let svc = GameService::new(conn);

    let first = svc
        .list(
            &user("alice"),
            &GameListParams {
                limit: 2,
                ..params(db_id)
            },
        )
        .await
        .unwrap();
    assert_eq!(first.games.len(), 2);
    assert_eq!(first.total, 5);
    assert_eq!(first.page, 0);
    assert_eq!(first.limit, 2);

    // Third page (0-based page 2) holds the single remaining row.
    let last = svc
        .list(
            &user("alice"),
            &GameListParams {
                page: 2,
                limit: 2,
                ..params(db_id)
            },
        )
        .await
        .unwrap();
    assert_eq!(last.games.len(), 1);
    assert_eq!(last.total, 5);

    // The pages are disjoint and ordered newest-first (descending id).
    assert!(first.games[0].id > first.games[1].id);
    assert!(first.games[1].id > last.games[0].id);
}

#[tokio::test]
async fn list_sorts_by_date_ascending_on_request() {
    let (conn, db_id) = db_for(Some("alice")).await;
    let dated = |w: &str, d: &str| {
        format!("[White \"{w}\"]\n[Black \"X\"]\n[Date \"{d}\"]\n[Result \"*\"]\n\n1. e4 *\n")
    };
    ingest_pgn(&conn, db_id, &dated("Newer", "2020.01.01"))
        .await
        .unwrap();
    ingest_pgn(&conn, db_id, &dated("Older", "1990.01.01"))
        .await
        .unwrap();
    let svc = GameService::new(conn);

    let page = svc
        .list(
            &user("alice"),
            &GameListParams {
                sort: GameSort::Date,
                dir: SortDir::Asc,
                ..params(db_id)
            },
        )
        .await
        .unwrap();
    // Ascending date: the 1990 game leads despite being inserted second.
    assert_eq!(page.games[0].white.as_deref(), Some("Older"));
    assert_eq!(page.games[1].white.as_deref(), Some("Newer"));
}

#[tokio::test]
async fn list_clamps_limit_to_max() {
    let (conn, db_id) = db_for(Some("alice")).await;
    ingest_pgn(&conn, db_id, SCHOLARS_MATE).await.unwrap();
    let svc = GameService::new(conn);

    // A limit above MAX_LIMIT must not error; it just caps at MAX_LIMIT.
    let page = svc
        .list(
            &user("alice"),
            &GameListParams {
                limit: MAX_LIMIT + 1000,
                ..params(db_id)
            },
        )
        .await
        .unwrap();
    assert_eq!(page.games.len(), 1);
    assert_eq!(page.limit, MAX_LIMIT);
}

#[tokio::test]
async fn list_rejects_invisible_database() {
    let (conn, alice_db) = db_for(Some("alice")).await;
    ingest_pgn(&conn, alice_db, SCHOLARS_MATE).await.unwrap();
    let svc = GameService::new(conn);

    // Bob cannot list games in alice's private database.
    let err = svc.list(&user("bob"), &params(alice_db)).await;
    assert!(matches!(err, Err(GameError::NotFound)));
}

#[tokio::test]
async fn list_sees_global_database() {
    let (conn, global_db) = db_for(None).await;
    ingest_pgn(&conn, global_db, SCHOLARS_MATE).await.unwrap();
    let svc = GameService::new(conn);

    let page = svc.list(&user("anyone"), &params(global_db)).await.unwrap();
    assert_eq!(page.games.len(), 1);
}

#[tokio::test]
async fn get_returns_game_with_pgn() {
    let (conn, db_id) = db_for(Some("alice")).await;
    let ingested = ingest_pgn(&conn, db_id, SCHOLARS_MATE)
        .await
        .unwrap()
        .unwrap();
    let svc = GameService::new(conn);

    let game = svc.get(&user("alice"), ingested.game_id).await.unwrap();
    assert_eq!(game.white.as_deref(), Some("Spassky"));
    assert_eq!(game.black.as_deref(), Some("Fischer"));
    assert_eq!(game.variant, "standard");
    let pgn = game.pgn.expect("PGN movetext stored");
    assert!(pgn.contains("Qxf7#"));
}

#[tokio::test]
async fn get_hides_game_in_another_users_database() {
    let (conn, alice_db) = db_for(Some("alice")).await;
    let ingested = ingest_pgn(&conn, alice_db, SCHOLARS_MATE)
        .await
        .unwrap()
        .unwrap();
    let svc = GameService::new(conn);

    let err = svc.get(&user("bob"), ingested.game_id).await;
    assert!(matches!(err, Err(GameError::NotFound)));
}

#[tokio::test]
async fn get_missing_game_is_not_found() {
    let (conn, _db_id) = db_for(Some("alice")).await;
    let svc = GameService::new(conn);
    let err = svc.get(&user("alice"), 9999).await;
    assert!(matches!(err, Err(GameError::NotFound)));
}
