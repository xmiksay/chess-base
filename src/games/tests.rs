//! Service-level tests over an in-memory SQLite DB: keyset pagination (cursor +
//! limit clamping), the visibility scope (own ∪ global, never another user's),
//! single-game fetch with PGN, and resolved player names.

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
async fn list_returns_games_oldest_first_with_player_names() {
    let (conn, db_id) = db_for(Some("alice")).await;
    ingest_pgn(&conn, db_id, SCHOLARS_MATE).await.unwrap();
    ingest_pgn(&conn, db_id, QUEENS_DRAW).await.unwrap();
    let svc = GameService::new(conn);

    let page = svc.list(&user("alice"), db_id, None, None).await.unwrap();
    assert_eq!(page.games.len(), 2);
    assert_eq!(page.next_cursor, None);
    assert_eq!(page.games[0].white.as_deref(), Some("Spassky"));
    assert_eq!(page.games[0].result.as_deref(), Some("1-0"));
    assert_eq!(page.games[1].white.as_deref(), Some("Carlsen"));
    // Oldest-first: ascending id.
    assert!(page.games[0].id < page.games[1].id);
}

#[tokio::test]
async fn list_paginates_by_keyset_cursor() {
    let (conn, db_id) = db_for(Some("alice")).await;
    for _ in 0..5 {
        ingest_pgn(&conn, db_id, SCHOLARS_MATE).await.unwrap();
    }
    let svc = GameService::new(conn);

    // First page of two: a cursor points past it.
    let first = svc
        .list(&user("alice"), db_id, None, Some(2))
        .await
        .unwrap();
    assert_eq!(first.games.len(), 2);
    let cursor = first.next_cursor.expect("more pages remain");
    assert_eq!(cursor, first.games[1].id);

    // Second page resumes strictly after the cursor.
    let second = svc
        .list(&user("alice"), db_id, Some(cursor), Some(2))
        .await
        .unwrap();
    assert_eq!(second.games.len(), 2);
    assert!(second.games[0].id > cursor);

    // Last page returns the remainder and no further cursor.
    let third = svc
        .list(
            &user("alice"),
            db_id,
            Some(second.next_cursor.unwrap()),
            Some(2),
        )
        .await
        .unwrap();
    assert_eq!(third.games.len(), 1);
    assert_eq!(third.next_cursor, None);
}

#[tokio::test]
async fn list_clamps_limit_to_max() {
    let (conn, db_id) = db_for(Some("alice")).await;
    ingest_pgn(&conn, db_id, SCHOLARS_MATE).await.unwrap();
    let svc = GameService::new(conn);

    // A limit above MAX_LIMIT must not error; it just caps at MAX_LIMIT.
    let page = svc
        .list(&user("alice"), db_id, None, Some(MAX_LIMIT + 1000))
        .await
        .unwrap();
    assert_eq!(page.games.len(), 1);
    assert_eq!(page.next_cursor, None);
}

#[tokio::test]
async fn list_rejects_invisible_database() {
    let (conn, alice_db) = db_for(Some("alice")).await;
    ingest_pgn(&conn, alice_db, SCHOLARS_MATE).await.unwrap();
    let svc = GameService::new(conn);

    // Bob cannot list games in alice's private database.
    let err = svc.list(&user("bob"), alice_db, None, None).await;
    assert!(matches!(err, Err(GameError::NotFound)));
}

#[tokio::test]
async fn list_sees_global_database() {
    let (conn, global_db) = db_for(None).await;
    ingest_pgn(&conn, global_db, SCHOLARS_MATE).await.unwrap();
    let svc = GameService::new(conn);

    let page = svc
        .list(&user("anyone"), global_db, None, None)
        .await
        .unwrap();
    assert_eq!(page.games.len(), 1);
}

#[tokio::test]
async fn get_returns_game_with_pgn() {
    let (conn, db_id) = db_for(Some("alice")).await;
    let ingested = ingest_pgn(&conn, db_id, SCHOLARS_MATE).await.unwrap();
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
    let ingested = ingest_pgn(&conn, alice_db, SCHOLARS_MATE).await.unwrap();
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
