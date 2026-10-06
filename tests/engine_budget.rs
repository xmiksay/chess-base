//! Whole-job engine budget (ADR-0054): long jobs stop at the deadline and return
//! partial results flagged `truncated` instead of erroring or running on. No
//! real engine is needed — an exhausted budget skips the engine entirely, and a
//! process that never finishes its UCI handshake (`cat`) stands in for a hung one.

use std::time::{Duration, Instant};

use sea_orm::{ActiveModelTrait, Set};

use chess_base::db::entities::databases;
use chess_base::db::{connect, DbConfig};
use chess_base::engine::{EngineConfig, EngineService, JobBudget, Limits};
use chess_base::position::STARTPOS_FEN;
use chess_base::review::review_game;
use chess_base::server::identity::CurrentUser;
use chess_base::studies::StudyService;

fn alice() -> CurrentUser {
    CurrentUser {
        id: "alice".to_string(),
        is_admin: false,
        public: false,
        read_only: false,
        global_only: false,
    }
}

/// An engine that must never be spawned: any attempt fails loudly.
fn missing_engine() -> EngineService {
    EngineService::new(EngineConfig::new("missing", "/nonexistent/engine"), 1)
}

fn sans(moves: &[&str]) -> Vec<String> {
    moves.iter().map(|m| m.to_string()).collect()
}

#[tokio::test]
async fn exhausted_budget_returns_an_empty_truncated_review() {
    let engine = missing_engine();
    let moves = sans(&["e4", "e5", "Nf3"]);
    let (review, truncated) = JobBudget::new(Duration::ZERO)
        .run(review_game(&engine, STARTPOS_FEN, "standard", &moves, 10))
        .await;
    let review = review.expect("budget exhaustion is not an error");
    assert!(review.truncated && truncated);
    assert!(review.moves.is_empty());
}

#[tokio::test]
async fn hung_engine_is_cut_off_at_the_deadline() {
    // `cat` never answers `uciok`, so the handshake would hang without a budget.
    let engine = EngineService::new(EngineConfig::new("hung", "cat"), 1);
    let started = Instant::now();
    let (lines, truncated) = JobBudget::new(Duration::from_millis(300))
        .run(engine.analyse_multi(STARTPOS_FEN, &Limits::depth(5), 2))
        .await;
    assert!(lines.expect("fallback, not an error").is_empty());
    assert!(truncated);
    assert!(started.elapsed() < Duration::from_secs(5));
}

#[tokio::test]
async fn exhausted_budget_leaves_the_study_untouched_and_flags_it() {
    let conn = connect(&DbConfig::in_memory()).await.unwrap();
    let db = databases::ActiveModel {
        owner_id: Set(Some("alice".to_string())),
        name: Set("Mine".to_string()),
        kind: Set("own".to_string()),
        ..Default::default()
    }
    .insert(&conn)
    .await
    .unwrap();
    let svc = StudyService::new(conn);
    let user = alice();
    let study = svc.create(&user, db.id, "Line", false).await.unwrap();
    let e4 = svc.add_move(&user, study.id, 0, "e4").await.unwrap();
    svc.add_move(&user, study.id, e4, "e5").await.unwrap();

    let engine = missing_engine();
    let (result, truncated) = JobBudget::new(Duration::ZERO)
        .run(svc.analyse_study(&engine, &user, study.id, 10))
        .await;
    let (model, stats) = result.expect("budget exhaustion is not an error");
    assert!(stats.truncated && truncated);
    assert_eq!(stats.nodes_analysed, 0);
    assert!(
        !model.tree_json.contains("\"eval\":{"),
        "no node gets an eval: {}",
        model.tree_json
    );
}
