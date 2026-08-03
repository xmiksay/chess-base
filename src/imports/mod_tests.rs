//! Tests for [`super`] (split out to keep `imports/mod.rs` under the file-size cap).

use super::*;
use crate::db::entities::games;
use crate::db::{connect, DbConfig};
use sea_orm::{ActiveModelTrait, Set};

const TWO_GAMES: &str = "[Event \"Game 1\"]\n[White \"Spassky\"]\n[Black \"Fischer\"]\n[Result \"1-0\"]\n\n1. e4 e5 2. Bc4 Nc6 3. Qh5 Nf6 4. Qxf7# 1-0\n\n[Event \"Game 2\"]\n[White \"Carlsen\"]\n[Black \"Caruana\"]\n[Result \"1/2-1/2\"]\n\n1. d4 d5 2. c4 e6 1/2-1/2\n";

fn user(id: &str) -> CurrentUser {
    CurrentUser {
        id: id.to_string(),
        is_admin: false,
        public: false,
    }
}

async fn service_with_db(owner: Option<&str>) -> (ImportService, i32) {
    let conn = connect(&DbConfig::in_memory()).await.unwrap();
    let db = databases::ActiveModel {
        owner_id: Set(owner.map(str::to_string)),
        name: Set("Games".to_string()),
        kind: Set("own".to_string()),
        ..Default::default()
    }
    .insert(&conn)
    .await
    .unwrap();
    (ImportService::new(conn.clone()), db.id)
}

#[test]
fn parses_known_sources_case_insensitively() {
    assert_eq!(ImportSource::parse("Lichess"), Some(ImportSource::Lichess));
    assert_eq!(
        ImportSource::parse("chesscom"),
        Some(ImportSource::ChessCom)
    );
    assert_eq!(
        ImportSource::parse("chess.com"),
        Some(ImportSource::ChessCom)
    );
    assert_eq!(ImportSource::parse("fics"), None);
}

#[tokio::test]
async fn import_pgn_ingests_every_game_into_an_owned_database() {
    let (svc, id) = service_with_db(Some("alice")).await;
    let summary = svc.import_pgn(&user("alice"), id, TWO_GAMES).await.unwrap();
    assert_eq!(summary.imported, 2);
    assert_eq!(summary.skipped, 0);
    assert_eq!(summary.duplicates, 0);
    assert!(summary.errors.is_empty());

    // The new games' ids come back (in PGN order) so a client can chain them.
    let stored = games::Entity::find().all(&svc.db).await.unwrap();
    assert_eq!(
        summary.game_ids,
        stored.iter().map(|g| g.id).collect::<Vec<_>>()
    );
}

#[tokio::test]
async fn import_pgn_reports_duplicates_on_reupload() {
    let (svc, id) = service_with_db(Some("alice")).await;
    svc.import_pgn(&user("alice"), id, TWO_GAMES).await.unwrap();

    let summary = svc.import_pgn(&user("alice"), id, TWO_GAMES).await.unwrap();
    assert_eq!(summary.imported, 0);
    assert_eq!(summary.duplicates, 2);
    assert!(summary.game_ids.is_empty());
    assert!(summary.errors.is_empty());
    assert_eq!(games::Entity::find().all(&svc.db).await.unwrap().len(), 2);
}

// One legal game then an illegal one (Black answers 1. e4 with another e4).
const ONE_GOOD_ONE_BAD: &str = "[Event \"Good\"]\n[White \"A\"]\n[Black \"B\"]\n[Result \"1-0\"]\n\n1. e4 e5 2. Bc4 Nc6 3. Qh5 Nf6 4. Qxf7# 1-0\n\n[Event \"Bad\"]\n[White \"C\"]\n[Black \"D\"]\n[Result \"*\"]\n\n1. e4 e4 *\n";

#[tokio::test]
async fn import_pgn_skips_a_bad_game_and_reports_it() {
    let (svc, id) = service_with_db(Some("alice")).await;
    let summary = svc
        .import_pgn(&user("alice"), id, ONE_GOOD_ONE_BAD)
        .await
        .unwrap();
    // Partial success is not an error: the good game lands, the bad one is
    // reported with a safe, indexed message (no leaked SQL / provider chain).
    assert_eq!(summary.imported, 1);
    assert_eq!(summary.skipped, 1);
    assert_eq!(summary.errors.len(), 1);
    assert!(summary.errors[0].starts_with("game 2:"));
    assert_eq!(games::Entity::find().all(&svc.db).await.unwrap().len(), 1);
}

#[tokio::test]
async fn import_pgn_rejects_empty_input() {
    let (svc, id) = service_with_db(Some("alice")).await;
    assert!(matches!(
        svc.import_pgn(&user("alice"), id, "  \n ")
            .await
            .unwrap_err(),
        ImportError::InvalidInput(_)
    ));
}

#[tokio::test]
async fn import_pgn_forbids_writing_another_users_database() {
    let (svc, id) = service_with_db(Some("alice")).await;
    assert!(matches!(
        svc.import_pgn(&user("bob"), id, TWO_GAMES)
            .await
            .unwrap_err(),
        ImportError::Forbidden
    ));
}

#[tokio::test]
async fn import_pgn_reports_a_missing_database() {
    let (svc, _) = service_with_db(Some("alice")).await;
    assert!(matches!(
        svc.import_pgn(&user("alice"), 9999, TWO_GAMES)
            .await
            .unwrap_err(),
        ImportError::NotFound
    ));
}

#[tokio::test]
async fn global_database_requires_admin_to_import() {
    let (svc, id) = service_with_db(None).await; // global (owner_id NULL)
                                                 // A non-admin is forbidden; the implicit admin succeeds.
    assert!(matches!(
        svc.import_pgn(&user("bob"), id, TWO_GAMES)
            .await
            .unwrap_err(),
        ImportError::Forbidden
    ));
    let summary = svc
        .import_pgn(&CurrentUser::local_admin(), id, TWO_GAMES)
        .await
        .unwrap();
    assert_eq!(summary.imported, 2);
    assert_eq!(summary.skipped, 0);
}

#[tokio::test]
async fn sync_requires_a_username_after_the_write_guard() {
    let (svc, id) = service_with_db(Some("alice")).await;
    assert!(matches!(
        svc.sync(&user("alice"), id, ImportSource::Lichess, "  ", None, false)
            .await
            .unwrap_err(),
        ImportError::InvalidInput(_)
    ));
}

/// A base URL nothing is listening on — the collector's very first request
/// fails fast with a connection error, deterministically and without
/// touching the network, so the sync-failure path is exercisable in CI.
async fn unreachable_base_url() -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    drop(listener);
    format!("http://{addr}")
}

#[tokio::test]
async fn full_resync_ignores_and_overwrites_the_stored_cursor() {
    use crate::collectors::SyncCursor;

    let (svc, id) = service_with_db(Some("alice")).await;
    let svc = svc.with_chesscom_base_url(unreachable_base_url().await);
    // Seed a stored cursor as if an earlier incremental sync had advanced
    // it, then confirm `full: true` starts from `SyncCursor::default()`
    // regardless — and overwrites the stored cursor once the run
    // completes (issue #197). The mock base URL fails the provider call
    // immediately; `sync` still persists whatever cursor it *attempted*
    // the run with before returning the error.
    cursor::save(
        &svc.db,
        id,
        "chesscom",
        "hikaru",
        &SyncCursor {
            last_month: Some("2099/01".to_string()),
            ..Default::default()
        },
        9,
        0,
    )
    .await
    .unwrap();

    let err = svc
        .sync(
            &user("alice"),
            id,
            ImportSource::ChessCom,
            "hikaru",
            None,
            true,
        )
        .await
        .unwrap_err();
    assert!(matches!(err, ImportError::Failed(_)));

    // The stored cursor is now the default (full ignored + overwrote
    // "2099/01"), not whatever an incremental sync would have loaded.
    let reloaded = cursor::load(&svc.db, id, "chesscom", "hikaru")
        .await
        .unwrap();
    assert_eq!(reloaded, SyncCursor::default());
}

#[tokio::test]
async fn a_failed_sync_still_persists_its_starting_cursor() {
    use crate::collectors::SyncCursor;

    // The incremental (non-full) counterpart of the test above: no cursor
    // is stored yet, the provider call fails immediately, and the default
    // cursor it started from is still persisted (issue #197) rather than
    // leaving `sync_cursors` empty forever after a failed first run.
    let (svc, id) = service_with_db(Some("alice")).await;
    let svc = svc.with_chesscom_base_url(unreachable_base_url().await);

    let err = svc
        .sync(
            &user("alice"),
            id,
            ImportSource::ChessCom,
            "hikaru",
            None,
            false,
        )
        .await
        .unwrap_err();
    assert!(matches!(err, ImportError::Failed(_)));

    let reloaded = cursor::load(&svc.db, id, "chesscom", "hikaru")
        .await
        .unwrap();
    assert_eq!(reloaded, SyncCursor::default());
}

#[tokio::test]
async fn different_usernames_in_the_same_database_never_share_a_cursor() {
    use crate::collectors::SyncCursor;

    let (svc, id) = service_with_db(Some("alice")).await;
    let alice_cursor = SyncCursor {
        last_month: Some("2024/06".to_string()),
        ..Default::default()
    };
    cursor::save(&svc.db, id, "chesscom", "alice", &alice_cursor, 20, 0)
        .await
        .unwrap();

    // Syncing a different username into the same collection must not
    // resume from alice's cursor (issue #197) — it starts from default.
    let loaded = cursor::load(&svc.db, id, "chesscom", "bob").await.unwrap();
    assert_eq!(loaded, SyncCursor::default());
    // alice's own cursor is unaffected by bob's existence.
    assert_eq!(
        cursor::load(&svc.db, id, "chesscom", "alice")
            .await
            .unwrap(),
        alice_cursor
    );
}
