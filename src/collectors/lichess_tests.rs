//! Tests for [`super`] (split out to keep `lichess.rs` under the file-size cap).

use super::*;
use crate::db::entities::{databases, games};
use crate::db::{connect, DbConfig};
use sea_orm::{ActiveModelTrait, EntityTrait, Set};

// Two-game export blob as Lichess streams it: `[Event ` per game, blank-line
// separated, second game one minute after the first.
const TWO_GAMES: &str = "[Event \"Rated blitz game\"]\n[Site \"https://lichess.org/abcd1234\"]\n[White \"alice\"]\n[Black \"bob\"]\n[Result \"1-0\"]\n[UTCDate \"2024.01.15\"]\n[UTCTime \"20:30:45\"]\n\n1. e4 e5 2. Bc4 Nc6 3. Qh5 Nf6 4. Qxf7# 1-0\n\n\n[Event \"Rated blitz game\"]\n[Site \"https://lichess.org/efgh5678\"]\n[White \"carol\"]\n[Black \"alice\"]\n[Result \"0-1\"]\n[UTCDate \"2024.01.15\"]\n[UTCTime \"20:31:45\"]\n\n1. d4 d5 2. c4 e6 0-1\n";

#[test]
fn builds_export_url_with_since() {
    let src = Lichess::new("DrNykterstein");
    assert_eq!(
        src.games_url(Some(1700000000000)),
        "https://lichess.org/api/games/user/DrNykterstein?pgnInJson=false&since=1700000000000"
    );
    assert_eq!(
        src.games_url(None),
        "https://lichess.org/api/games/user/DrNykterstein?pgnInJson=false"
    );
    assert_eq!(src.kind(), "lichess");
}

#[test]
fn first_sync_has_no_since_then_resumes_at_last_game() {
    // No cursor ⇒ full sync.
    assert_eq!(since_param(&SyncCursor::default()), None);
    // With a cursor ⇒ resume *at* the last synced game's second (not past it),
    // so games sharing that second are not skipped. The boundary game is
    // deduped by ingest rather than skipped by the cursor (issue #95).
    let cursor = SyncCursor {
        last_game_ms: Some(1_705_350_645_000),
        ..Default::default()
    };
    assert_eq!(since_param(&cursor), Some(1_705_350_645_000));
}

#[test]
fn cursor_advances_to_newest_game_only() {
    assert_eq!(advance_ms(None, Some(100)), Some(100));
    assert_eq!(advance_ms(Some(100), Some(50)), Some(100)); // older ignored
    assert_eq!(advance_ms(Some(100), Some(200)), Some(200));
    assert_eq!(advance_ms(Some(100), None), Some(100)); // untimed game
    assert_eq!(advance_ms(None, None), None);
}

#[test]
fn backoff_is_at_least_one_minute() {
    assert_eq!(backoff_delay(None, MIN_BACKOFF), Duration::from_secs(60));
    // Server asks for less than the mandated minimum ⇒ floored to 60s.
    assert_eq!(
        backoff_delay(Some(10), MIN_BACKOFF),
        Duration::from_secs(60)
    );
    // Server asks for longer ⇒ honoured.
    assert_eq!(
        backoff_delay(Some(120), MIN_BACKOFF),
        Duration::from_secs(120)
    );
}

#[test]
fn splits_stream_into_individual_games() {
    let games = split_games(TWO_GAMES);
    assert_eq!(games.len(), 2);
    assert!(games[0].contains("Qxf7#"));
    assert!(games[1].contains("carol"));
    assert!(games[1].starts_with("[Event "));
}

#[test]
fn trailing_offset_withholds_the_last_partial_game() {
    // One game so far ⇒ nothing provably complete.
    let one = b"[Event \"x\"]\n\n1. e4 *";
    assert_eq!(trailing_game_offset(one), None);
    // Two markers ⇒ the second game's start is the withhold point.
    let split = trailing_game_offset(TWO_GAMES.as_bytes()).unwrap();
    assert!(TWO_GAMES[split..].starts_with("[Event "));
    assert!(TWO_GAMES[..split].contains("Qxf7#"));
    assert!(!TWO_GAMES[..split].contains("carol"));
}

#[test]
fn parses_game_timestamp_from_utc_tags() {
    let games = split_games(TWO_GAMES);
    // 2024-01-15 20:30:45 UTC.
    assert_eq!(game_epoch_ms(&games[0]), Some(1_705_350_645_000));
    // One minute later.
    assert_eq!(game_epoch_ms(&games[1]), Some(1_705_350_705_000));
    assert_eq!(game_epoch_ms("[Event \"x\"]\n\n1. e4 *"), None);
}

async fn own_database() -> (DatabaseConnection, i32) {
    let conn = connect(&DbConfig::in_memory()).await.unwrap();
    let db = databases::ActiveModel {
        owner_id: Set(Some("alice".to_string())),
        name: Set("Alice's lichess".to_string()),
        kind: Set("lichess".to_string()),
        ..Default::default()
    }
    .insert(&conn)
    .await
    .unwrap();
    (conn, db.id)
}

#[tokio::test]
async fn ingests_a_blob_and_advances_cursor_to_newest_game() {
    let (conn, database_id) = own_database().await;
    let mut cursor = SyncCursor::default();
    let (mut imported, mut duplicates) = (0, 0);

    ingest_blob(
        &conn,
        database_id,
        TWO_GAMES,
        &mut cursor,
        &mut imported,
        &mut duplicates,
    )
    .await
    .unwrap();

    assert_eq!(imported, 2);
    assert_eq!(duplicates, 0);
    assert_eq!(games::Entity::find().all(&conn).await.unwrap().len(), 2);
    // Cursor sits on the newer (second) game.
    assert_eq!(cursor.last_game_ms, Some(1_705_350_705_000));
    // A re-sync resumes *at* it; the boundary game is deduped, not skipped.
    assert_eq!(since_param(&cursor), Some(1_705_350_705_000));
}

#[tokio::test]
async fn resync_with_only_new_games_appends_without_rewinding_cursor() {
    let (conn, database_id) = own_database().await;
    let mut cursor = SyncCursor::default();
    let (mut imported, mut duplicates) = (0, 0);
    ingest_blob(
        &conn,
        database_id,
        TWO_GAMES,
        &mut cursor,
        &mut imported,
        &mut duplicates,
    )
    .await
    .unwrap();

    // A later sync returns a single, newer game; the cursor only moves forward.
    let newer = "[Event \"Rated blitz game\"]\n[White \"alice\"]\n[Black \"dave\"]\n[Result \"1-0\"]\n[UTCDate \"2024.01.16\"]\n[UTCTime \"09:00:00\"]\n\n1. e4 e5 2. Bc4 Nc6 3. Qh5 Nf6 4. Qxf7# 1-0\n";
    ingest_blob(
        &conn,
        database_id,
        newer,
        &mut cursor,
        &mut imported,
        &mut duplicates,
    )
    .await
    .unwrap();

    assert_eq!(imported, 3);
    assert_eq!(games::Entity::find().all(&conn).await.unwrap().len(), 3);
    assert_eq!(cursor.last_game_ms, Some(1_705_395_600_000)); // 2024-01-16 09:00 UTC
}

#[tokio::test]
async fn re_ingesting_the_same_blob_imports_nothing() {
    let (conn, database_id) = own_database().await;
    let mut cursor = SyncCursor::default();
    let (mut imported, mut duplicates) = (0, 0);
    ingest_blob(
        &conn,
        database_id,
        TWO_GAMES,
        &mut cursor,
        &mut imported,
        &mut duplicates,
    )
    .await
    .unwrap();

    // The boundary re-fetch a resumed sync produces: the same two games (same
    // Lichess permalinks) are deduped, so nothing is added and the cursor
    // still tracks the newest game.
    ingest_blob(
        &conn,
        database_id,
        TWO_GAMES,
        &mut cursor,
        &mut imported,
        &mut duplicates,
    )
    .await
    .unwrap();

    assert_eq!(imported, 2);
    assert_eq!(duplicates, 2);
    assert_eq!(games::Entity::find().all(&conn).await.unwrap().len(), 2);
    assert_eq!(cursor.last_game_ms, Some(1_705_350_705_000));
}

#[tokio::test]
async fn sync_failure_reports_the_cursor_advanced_before_the_error() {
    // A transport that serves the two-game export, then always 500s — the
    // stand-in for a Lichess 429 exhausting its retries or a dropped stream
    // mid-run. `Lichess::sync_with` can't easily be driven against a mock
    // server here, so this exercises the same partial-progress contract at
    // the `ingest_blob` level the collector relies on: a failure captured via
    // `&mut` accumulators must leave prior games' progress intact.
    let (conn, database_id) = own_database().await;
    let mut cursor = SyncCursor::default();
    let (mut imported, mut duplicates) = (0, 0);
    ingest_blob(
        &conn,
        database_id,
        TWO_GAMES,
        &mut cursor,
        &mut imported,
        &mut duplicates,
    )
    .await
    .unwrap();

    // Simulate the run failing on a second, illegal-move blob: `ingest_blob`
    // returns `Err`, but the cursor/imported already reflect the first blob.
    let bad = "[Event \"x\"]\n[White \"a\"]\n[Black \"b\"]\n[Result \"*\"]\n\n1. e4 e4 *\n";
    let err = ingest_blob(
        &conn,
        database_id,
        bad,
        &mut cursor,
        &mut imported,
        &mut duplicates,
    )
    .await;
    assert!(err.is_err());
    assert_eq!(imported, 2);
    assert_eq!(cursor.last_game_ms, Some(1_705_350_705_000));
}
