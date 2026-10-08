//! Tests for [`super`]: id parsing (pure) and the lichess.org → Masters
//! fallback against a local fake of both upstreams.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use axum::{extract::Path, http::StatusCode, routing::get, Router};
use sea_orm::{ActiveModelTrait, Set};

use super::*;
use crate::db::entities::databases;
use crate::db::{connect, DbConfig};

const LICHESS_PGN: &str = "[Event \"Rated blitz game\"]\n[Site \"https://lichess.org/LiChEsS1\"]\n[White \"a\"]\n[Black \"b\"]\n[Result \"1-0\"]\n\n1. e4 e5 2. Bc4 Nc6 3. Qh5 Nf6 4. Qxf7# 1-0\n";
const MASTERS_PGN: &str = "[Event \"Candidates\"]\n[White \"Tal, M.\"]\n[Black \"Smyslov, V.\"]\n[Result \"1/2-1/2\"]\n\n1. d4 d5 2. c4 e6 1/2-1/2\n";

#[test]
fn parses_bare_ids_and_lichess_urls() {
    for (input, id) in [
        ("LiChEsS1", "LiChEsS1"),
        ("  LiChEsS1  ", "LiChEsS1"),
        ("LiChEsS1abcd", "LiChEsS1"),
        ("https://lichess.org/LiChEsS1", "LiChEsS1"),
        ("lichess.org/LiChEsS1/black", "LiChEsS1"),
        ("https://lichess.org/LiChEsS1abcd#23", "LiChEsS1"),
        (
            "https://lichess.org/game/export/LiChEsS1?clocks=true",
            "LiChEsS1",
        ),
        (
            "https://explorer.lichess.ovh/masters/pgn/MaStErS1",
            "MaStErS1",
        ),
    ] {
        assert_eq!(parse_game_ref(input).expect(input), id, "{input}");
    }
}

#[test]
fn rejects_non_ids_and_foreign_urls() {
    for input in [
        "",
        "abc",
        "../etc/pa",
        "abcd-123",
        "https://www.chess.com/game/live/123456789012",
        "https://lichess.org/@/someuser",
    ] {
        assert!(
            matches!(parse_game_ref(input), Err(ImportError::InvalidInput(_))),
            "{input}"
        );
    }
}

/// Fake lichess.org + explorer on one port; counts every request it serves.
async fn fake_upstream() -> (String, Arc<AtomicUsize>) {
    let hits = Arc::new(AtomicUsize::new(0));
    let (h1, h2) = (hits.clone(), hits.clone());
    let app = Router::new()
        .route(
            "/game/export/{id}",
            get(move |Path(id): Path<String>| async move {
                h1.fetch_add(1, Ordering::SeqCst);
                match id.as_str() {
                    "LiChEsS1" => (StatusCode::OK, LICHESS_PGN),
                    "RaTeLiMt" => (StatusCode::TOO_MANY_REQUESTS, ""),
                    _ => (StatusCode::NOT_FOUND, ""),
                }
            }),
        )
        .route(
            "/masters/pgn/{id}",
            get(move |Path(id): Path<String>| async move {
                h2.fetch_add(1, Ordering::SeqCst);
                match id.as_str() {
                    "MaStErS1" => (StatusCode::OK, MASTERS_PGN),
                    _ => (StatusCode::NOT_FOUND, ""),
                }
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await });
    (base, hits)
}

async fn setup(owner: Option<&str>) -> (ImportService, i32, MastersClient, Arc<AtomicUsize>) {
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
    let (base, hits) = fake_upstream().await;
    let mut svc = ImportService::new(conn);
    svc.lichess_base_url = Some(base.clone());
    let masters = MastersClient::new(base, "token").unwrap();
    (svc, db.id, masters, hits)
}

fn user(id: &str) -> CurrentUser {
    CurrentUser {
        id: id.to_string(),
        is_admin: false,
        public: false,
        read_only: false,
        global_only: false,
    }
}

#[tokio::test]
async fn imports_a_lichess_org_game_then_reports_the_duplicate() {
    let (svc, db, masters, _) = setup(Some("alice")).await;
    let url = "https://lichess.org/LiChEsS1/black";
    let first = svc
        .import_lichess_game(&user("alice"), db, url, Some(&masters))
        .await
        .unwrap();
    assert_eq!(
        (first.imported, first.duplicates, first.game_ids.len()),
        (1, 0, 1)
    );
    let again = svc
        .import_lichess_game(&user("alice"), db, "LiChEsS1", Some(&masters))
        .await
        .unwrap();
    assert_eq!((again.imported, again.duplicates), (0, 1));
}

#[tokio::test]
async fn a_lichess_org_miss_falls_back_to_masters() {
    let (svc, db, masters, _) = setup(Some("alice")).await;
    let summary = svc
        .import_lichess_game(&user("alice"), db, "MaStErS1", Some(&masters))
        .await
        .unwrap();
    assert_eq!(summary.imported, 1);
}

#[tokio::test]
async fn a_miss_on_both_or_without_masters_is_a_clear_failure() {
    let (svc, db, masters, _) = setup(Some("alice")).await;
    let err = svc
        .import_lichess_game(&user("alice"), db, "NoSuChId", Some(&masters))
        .await
        .unwrap_err();
    assert!(
        matches!(&err, ImportError::Failed(m) if m.contains("Masters database")),
        "{err}"
    );
    let err = svc
        .import_lichess_game(&user("alice"), db, "MaStErS1", None)
        .await
        .unwrap_err();
    assert!(
        matches!(&err, ImportError::Failed(m) if m.contains("LICHESS_TOKEN")),
        "{err}"
    );
}

#[tokio::test]
async fn a_rate_limit_is_reported_without_trying_masters() {
    let (svc, db, masters, hits) = setup(Some("alice")).await;
    let err = svc
        .import_lichess_game(&user("alice"), db, "RaTeLiMt", Some(&masters))
        .await
        .unwrap_err();
    assert!(
        matches!(&err, ImportError::Failed(m) if m.contains("rate limit")),
        "{err}"
    );
    assert_eq!(hits.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn a_forbidden_caller_never_reaches_upstream() {
    let (svc, db, masters, hits) = setup(Some("alice")).await;
    let err = svc
        .import_lichess_game(&user("mallory"), db, "LiChEsS1", Some(&masters))
        .await
        .unwrap_err();
    assert!(matches!(err, ImportError::Forbidden));
    assert_eq!(hits.load(Ordering::SeqCst), 0);
}
