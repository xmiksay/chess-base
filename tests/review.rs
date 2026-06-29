//! Integration tests for the full-game engine review route
//! (`POST /api/games/{id}/analyse`, Mode A of issue #119), driven end-to-end
//! through the real router in local mode (implicit admin, global database).
//!
//! The engine-driven happy path is gated on a real UCI binary via
//! `CHESS_BASE_TEST_ENGINE` (like `tests/engine.rs`); the configuration-gap and
//! not-found paths need no engine and always run.
//!
//!     CHESS_BASE_TEST_ENGINE=$(which stockfish) cargo test --test review

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use http_body_util::BodyExt;
use serde_json::Value;
use tower::ServiceExt;

use chess_base::db::entities::databases;
use chess_base::db::{connect, DbConfig};
use chess_base::engine::{EngineConfig, EngineService};
use chess_base::ingest_pgn;
use chess_base::server::{build_router, AppState, Mode};
use sea_orm::{ActiveModelTrait, DatabaseConnection, Set};

const SCHOLARS_MATE: &str =
    "[White \"Spassky\"]\n[Black \"Fischer\"]\n[Result \"1-0\"]\n\n1. e4 e5 2. Bc4 Nc6 3. Qh5 Nf6 4. Qxf7# 1-0\n";

/// Build a local-mode app over an in-memory DB with the given engine wired on.
async fn app_with(engine: Option<Arc<EngineService>>) -> (Router, DatabaseConnection) {
    let db = connect(&DbConfig::in_memory()).await.unwrap();
    let app = build_router(AppState {
        db: db.clone(),
        mode: Mode::Local,
        engine_service: engine,
        llm_provider: None,
    });
    (app, db)
}

/// Seed a global database (owner NULL ⇒ visible to the local admin) with `pgn`
/// and return the first game's id.
async fn seed_game(db: &DatabaseConnection, pgn: &str) -> i32 {
    let database = databases::ActiveModel {
        owner_id: Set(None),
        name: Set("Masters".into()),
        kind: Set("master".into()),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap();
    let ingested = ingest_pgn(db, database.id, pgn).await.unwrap();
    ingested.expect("game ingested").game_id
}

/// A pool over a never-spawned engine: enough for paths that bail before
/// touching the engine (not-found), never actually launched.
fn dummy_engine() -> Arc<EngineService> {
    Arc::new(EngineService::new(
        EngineConfig::new("dummy", "/nonexistent/engine"),
        1,
    ))
}

async fn analyse(app: &Router, uri: &str) -> (StatusCode, Value) {
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(uri)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    let body = serde_json::from_slice(&resp.into_body().collect().await.unwrap().to_bytes())
        .unwrap_or(Value::Null);
    (status, body)
}

#[tokio::test]
async fn analyse_without_an_engine_is_service_unavailable() {
    let (app, db) = app_with(None).await;
    let id = seed_game(&db, SCHOLARS_MATE).await;
    let (status, _) = analyse(&app, &format!("/api/games/{id}/analyse")).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn analyse_missing_game_is_not_found() {
    // Engine present so the route gets past the config gate to the game lookup,
    // which fails first — the dummy engine is never spawned.
    let (app, _db) = app_with(Some(dummy_engine())).await;
    let (status, _) = analyse(&app, "/api/games/9999/analyse").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

fn engine_path() -> Option<String> {
    match std::env::var("CHESS_BASE_TEST_ENGINE") {
        Ok(p) if !p.trim().is_empty() => Some(p),
        _ => {
            eprintln!("skipping: set CHESS_BASE_TEST_ENGINE to a UCI engine binary to run");
            None
        }
    }
}

#[tokio::test]
async fn analyses_a_full_game_with_classifications_and_summary() {
    let Some(path) = engine_path() else { return };
    let engine = Arc::new(EngineService::new(EngineConfig::new("test", path), 1));
    let (app, db) = app_with(Some(engine)).await;
    let id = seed_game(&db, SCHOLARS_MATE).await;

    // Shallow depth keeps the test quick; the classification still resolves.
    let (status, body) = analyse(&app, &format!("/api/games/{id}/analyse?depth=10")).await;
    assert_eq!(status, StatusCode::OK, "body: {body}");

    let moves = body["moves"].as_array().expect("moves array");
    assert_eq!(moves.len(), 7, "Scholar's mate is seven plies");
    // The final move is checkmate: White's eval saturates to a mate score.
    let mate = moves.last().unwrap();
    assert_eq!(mate["san"], "Qxf7#");
    assert_eq!(mate["mate"], 1);
    // Every move carries a classification and a (possibly empty) explanation.
    for m in moves {
        assert!(m["classification"].is_string(), "classification on {m}");
        assert!(m["explanation"].is_string(), "explanation on {m}");
    }
    // Black walked into a mate, so its accuracy must trail White's.
    let white_acc = body["summary"]["white"]["accuracy"].as_f64().unwrap();
    let black_acc = body["summary"]["black"]["accuracy"].as_f64().unwrap();
    assert!(
        white_acc > black_acc,
        "white {white_acc} vs black {black_acc}"
    );
}
