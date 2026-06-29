//! Tests for the streaming bulk master-DB importer (issue #4).

use std::io::Cursor;

use sea_orm::EntityTrait;

use super::{find_or_create_master, BulkImporter};
use crate::db::entities::{databases, games, position_index};
use crate::db::{connect, DbConfig};

const GAME_ONE: &str = "[Event \"Game 1\"]\n[White \"Spassky\"]\n[Black \"Fischer\"]\n[Result \"1-0\"]\n[Date \"1972.07.11\"]\n\n1. e4 e5 2. Bc4 Nc6 3. Qh5 Nf6 4. Qxf7# 1-0\n";
const GAME_TWO: &str = "[Event \"Game 2\"]\n[White \"Carlsen\"]\n[Black \"Caruana\"]\n[Result \"1/2-1/2\"]\n[Date \"2018.11.28\"]\n\n1. d4 d5 2. c4 e6 1/2-1/2\n";

fn two_games() -> String {
    format!("{GAME_ONE}\n{GAME_TWO}")
}

async fn master_db() -> (sea_orm::DatabaseConnection, i32) {
    let conn = connect(&DbConfig::in_memory()).await.unwrap();
    let id = find_or_create_master(&conn, "Master Database")
        .await
        .unwrap();
    (conn, id)
}

#[tokio::test]
async fn imports_every_game_and_indexes_positions() {
    let (conn, id) = master_db().await;
    let report = BulkImporter::new()
        .import_reader(&conn, id, Cursor::new(two_games()))
        .await
        .unwrap();

    assert_eq!(report.imported, 2);
    assert_eq!(report.duplicates, 0);
    assert_eq!(report.errors, 0);
    assert_eq!(games::Entity::find().all(&conn).await.unwrap().len(), 2);
    // The mainline of both games is replayed into the position index.
    assert!(!position_index::Entity::find()
        .all(&conn)
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn skips_duplicate_games_within_one_file() {
    let (conn, id) = master_db().await;
    // The same game twice plus a distinct one: the second copy is a duplicate.
    let blob = format!("{GAME_ONE}\n{GAME_ONE}\n{GAME_TWO}");
    let report = BulkImporter::new()
        .import_reader(&conn, id, Cursor::new(blob))
        .await
        .unwrap();

    assert_eq!(report.imported, 2);
    assert_eq!(report.duplicates, 1);
    assert_eq!(games::Entity::find().all(&conn).await.unwrap().len(), 2);
}

#[tokio::test]
async fn re_running_is_restartable_and_imports_nothing_new() {
    let (conn, id) = master_db().await;
    let importer = BulkImporter::new();

    let first = importer
        .import_reader(&conn, id, Cursor::new(two_games()))
        .await
        .unwrap();
    assert_eq!(first.imported, 2);

    // A second run over the same file re-skips every game via the stored hash.
    let second = importer
        .import_reader(&conn, id, Cursor::new(two_games()))
        .await
        .unwrap();
    assert_eq!(second.imported, 0);
    assert_eq!(second.duplicates, 2);
    assert_eq!(games::Entity::find().all(&conn).await.unwrap().len(), 2);
}

#[tokio::test]
async fn imports_a_zst_compressed_file() {
    let (conn, id) = master_db().await;

    let path =
        std::env::temp_dir().join(format!("chess-base-bulk-{}.pgn.zst", uuid::Uuid::new_v4()));
    let compressed = zstd::encode_all(Cursor::new(two_games()), 3).unwrap();
    std::fs::write(&path, compressed).unwrap();

    let report = BulkImporter::new()
        .import_path(&conn, id, &path)
        .await
        .unwrap();
    std::fs::remove_file(&path).ok();

    assert_eq!(report.imported, 2);
    assert_eq!(games::Entity::find().all(&conn).await.unwrap().len(), 2);
}

#[tokio::test]
async fn commits_across_multiple_batches() {
    let (conn, id) = master_db().await;
    let blob = format!(
        "{GAME_ONE}\n{GAME_TWO}\n{}",
        GAME_ONE
            .replace("Spassky", "Tal")
            .replace("Fischer", "Botvinnik")
    );
    // batch_size 1 forces a commit per game, exercising the flush boundary.
    let report = BulkImporter::new()
        .with_batch_size(1)
        .import_reader(&conn, id, Cursor::new(blob))
        .await
        .unwrap();

    assert_eq!(report.imported, 3);
    assert_eq!(games::Entity::find().all(&conn).await.unwrap().len(), 3);
}

#[tokio::test]
async fn skips_an_illegal_game_and_continues() {
    let (conn, id) = master_db().await;
    // Black answers 1. e4 with another e4 — illegal, skip-and-continue.
    let bad = "[Event \"Bad\"]\n[White \"C\"]\n[Black \"D\"]\n[Result \"*\"]\n\n1. e4 e4 *\n";
    let blob = format!("{GAME_ONE}\n{bad}\n{GAME_TWO}");
    let report = BulkImporter::new()
        .import_reader(&conn, id, Cursor::new(blob))
        .await
        .unwrap();

    assert_eq!(report.imported, 2);
    assert_eq!(report.errors, 1);
    assert_eq!(games::Entity::find().all(&conn).await.unwrap().len(), 2);
}

#[tokio::test]
async fn find_or_create_master_is_idempotent() {
    let conn = connect(&DbConfig::in_memory()).await.unwrap();
    let first = find_or_create_master(&conn, "Master Database")
        .await
        .unwrap();
    let second = find_or_create_master(&conn, "Master Database")
        .await
        .unwrap();
    assert_eq!(first, second);

    let model = databases::Entity::find_by_id(first)
        .one(&conn)
        .await
        .unwrap()
        .unwrap();
    assert!(model.owner_id.is_none(), "master database is global");
    assert_eq!(model.kind, "master");
    assert_eq!(model.index_depth, Some(databases::MASTER_INDEX_DEPTH));
    // Only one row exists despite two calls.
    assert_eq!(databases::Entity::find().all(&conn).await.unwrap().len(), 1);
}
