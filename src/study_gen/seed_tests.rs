//! Tests for the LLM-free study seed seam (#155). A tree is built from injected
//! fakes — a fake evaluator + continuation source for the opening path, a fake
//! multi-PV analyzer for the danger path — then persisted against a real in-memory
//! SQLite study service. No language model is involved anywhere. Split out to keep
//! `seed.rs` under the project's 500-line file cap.

use super::*;

use std::collections::HashMap;

use anyhow::Result;
use async_trait::async_trait;
use sea_orm::{ActiveModelTrait, Set};

use crate::db::entities::databases;
use crate::db::{connect, DbConfig};
use crate::engine::{Analysis, Score};
use crate::pgn_tree::MoveTree;
use crate::position::{replay, CastlingMode, STARTPOS_FEN};
use crate::search::report::MoveReport;
use crate::study_gen::spine::Side;
use crate::study_gen::tree::{build_tree, ContinuationSource, Evaluator, TreeConfig};
use crate::study_gen::{walk_danger_spine, MultiAnalyzer, SpineConfig};

const STD: CastlingMode = CastlingMode::Standard;

// --- Fakes ----------------------------------------------------------------

struct FakeEval(HashMap<String, Score>);
struct FakeAnalyzer(HashMap<String, Vec<Analysis>>);
struct FakeStats(HashMap<String, Vec<MoveReport>>);

#[async_trait]
impl Evaluator for FakeEval {
    async fn eval(&self, fen: &str) -> Result<Option<Score>> {
        Ok(self.0.get(fen).copied())
    }
}

#[async_trait]
impl MultiAnalyzer for FakeAnalyzer {
    async fn analyse_multi(&self, fen: &str) -> Result<Vec<Analysis>> {
        Ok(self.0.get(fen).cloned().unwrap_or_default())
    }
}

#[async_trait]
impl ContinuationSource for FakeStats {
    async fn continuations(&self, fen: &str) -> Result<Vec<MoveReport>> {
        Ok(self.0.get(fen).cloned().unwrap_or_default())
    }
}

// --- Builders -------------------------------------------------------------

fn fen_after(sans: &[&str]) -> String {
    if sans.is_empty() {
        return STARTPOS_FEN.to_string();
    }
    replay(STARTPOS_FEN, sans, STD)
        .unwrap()
        .last()
        .unwrap()
        .fen
        .clone()
}

fn line(uci: &str, cp: i32) -> Analysis {
    Analysis {
        bestmove: uci.to_string(),
        ponder: None,
        score: Some(Score::Cp { value: cp }),
        depth: None,
        pv: vec![uci.to_string()],
    }
}

fn report(san: &str, frequency: f64) -> MoveReport {
    MoveReport {
        san: san.to_string(),
        count: (frequency * 100.0) as u64,
        white: 0,
        draws: 0,
        black: 0,
        frequency,
        score: 0.5,
    }
}

/// From the start: e4 and d4; after e4, c5. Mirrors the generator fixture.
fn opening_fixture() -> (FakeEval, FakeStats) {
    let mut conts = HashMap::new();
    conts.insert(fen_after(&[]), vec![report("e4", 0.6), report("d4", 0.3)]);
    conts.insert(fen_after(&["e4"]), vec![report("c5", 0.7)]);

    let mut evals = HashMap::new();
    evals.insert(fen_after(&[]), Score::Cp { value: 20 });
    evals.insert(fen_after(&["e4"]), Score::Cp { value: -30 });
    evals.insert(fen_after(&["d4"]), Score::Cp { value: 40 });
    evals.insert(fen_after(&["e4", "c5"]), Score::Cp { value: 25 });
    (FakeEval(evals), FakeStats(conts))
}

fn tree_config() -> TreeConfig {
    TreeConfig {
        max_depth: 2,
        max_children: 5,
        max_nodes: 100,
        min_frequency: 0.0,
        eval_margin_cp: 10_000,
    }
}

/// A White repertoire: 1.e4, prepared only against 1...c5.
fn white_spine() -> MoveTree {
    let mut t = MoveTree::new();
    let e4 = t.add_move(t.root, "e4");
    let c5 = t.add_move(e4, "c5");
    t.add_move(c5, "Nf3");
    t
}

/// After 1.e4 the engine sees c5 hold −10 but e5 collapse to −300: a weapon trap
/// tagged on e4. c5 is on-book; nothing else is offered.
fn danger_fixture() -> (FakeAnalyzer, FakeStats) {
    let mut an = HashMap::new();
    an.insert(
        fen_after(&["e4"]),
        vec![line("c7c5", -10), line("e7e5", -300)],
    );
    let mut stats = HashMap::new();
    stats.insert(fen_after(&["e4"]), vec![report("c5", 0.5)]);
    (FakeAnalyzer(an), FakeStats(stats))
}

/// Fresh in-memory DB seeded with one database row owned by `alice`.
async fn setup() -> (StudyService, i32) {
    let conn = connect(&DbConfig::in_memory()).await.unwrap();
    let db = databases::ActiveModel {
        owner_id: Set(Some("alice".to_string())),
        name: Set("Alice's games".to_string()),
        kind: Set("own".to_string()),
        ..Default::default()
    }
    .insert(&conn)
    .await
    .unwrap();
    (StudyService::new(conn), db.id)
}

fn alice() -> CurrentUser {
    CurrentUser {
        id: "alice".to_string(),
        is_admin: false,
    }
}

fn seed_params(database_id: i32, global: bool) -> SeedParams {
    SeedParams {
        database_id,
        name: "Seeded".to_string(),
        global,
    }
}

// --- Tests ----------------------------------------------------------------

#[tokio::test]
async fn seeds_an_opening_tree_into_a_persisted_study() {
    let (svc, db_id) = setup().await;
    let (eval, stats) = opening_fixture();
    let tree = build_tree(&eval, &stats, STARTPOS_FEN, &tree_config(), STD)
        .await
        .unwrap();

    let outcome = seed_study_from_tree(&svc, &alice(), &tree, &seed_params(db_id, false))
        .await
        .expect("seed succeeds");

    // A study was saved, owned by the caller, with one move-tree node per tree node.
    assert_eq!(outcome.study.owner_id.as_deref(), Some("alice"));
    assert_eq!(outcome.node_count, tree.nodes.len());
    assert!(outcome.node_count >= 3); // root + e4 + d4 (+ c5)

    // The persisted study is readable and structurally matches the built tree; a
    // startpos seed records no `start_fen`.
    let saved = svc.get(&alice(), outcome.study.id).await.unwrap();
    let saved_tree: MoveTree = serde_json::from_str(&saved.tree_json).unwrap();
    assert_eq!(saved_tree.nodes.len(), tree.nodes.len());
    assert_eq!(saved_tree.start_fen, None);
    // No LLM ran: the seeded skeleton carries no comments.
    assert!(saved_tree.nodes.iter().all(|n| n.comment.is_none()));
}

#[tokio::test]
async fn a_non_startpos_seed_records_its_start_fen() {
    let (svc, db_id) = setup().await;
    // Grow the tree from the position after 1.e4 — a set-up start (ADR-0028).
    let start = fen_after(&["e4"]);
    let mut conts = HashMap::new();
    conts.insert(start.clone(), vec![report("c5", 0.7)]);
    let mut evals = HashMap::new();
    evals.insert(start.clone(), Score::Cp { value: -30 });
    evals.insert(fen_after(&["e4", "c5"]), Score::Cp { value: 25 });
    let tree = build_tree(
        &FakeEval(evals),
        &FakeStats(conts),
        &start,
        &tree_config(),
        STD,
    )
    .await
    .unwrap();

    let outcome = seed_study_from_tree(&svc, &alice(), &tree, &seed_params(db_id, false))
        .await
        .unwrap();

    let saved = svc.get(&alice(), outcome.study.id).await.unwrap();
    let saved_tree: MoveTree = serde_json::from_str(&saved.tree_json).unwrap();
    assert_eq!(saved_tree.start_fen.as_deref(), Some(start.as_str()));
}

#[tokio::test]
async fn seeds_a_danger_tree_into_a_persisted_study() {
    let (svc, db_id) = setup().await;
    let (an, stats) = danger_fixture();
    let config = SpineConfig {
        our_side: Side::White,
        max_depth: 2,
        ..SpineConfig::default()
    };
    let danger = walk_danger_spine(&an, &stats, &white_spine(), STARTPOS_FEN, &config, STD)
        .await
        .unwrap();

    let outcome = seed_study_from_danger(&svc, &alice(), &danger, &seed_params(db_id, false))
        .await
        .expect("seed succeeds");

    assert_eq!(outcome.study.owner_id.as_deref(), Some("alice"));
    assert_eq!(outcome.node_count, danger.nodes.len());

    // The persisted study mirrors the danger tree shape and reaches e4 by a legal
    // move (correct-by-construction), with no annotations.
    let saved = svc.get(&alice(), outcome.study.id).await.unwrap();
    let saved_tree: MoveTree = serde_json::from_str(&saved.tree_json).unwrap();
    assert_eq!(saved_tree.nodes.len(), danger.nodes.len());
    assert_eq!(saved_tree.nodes[1].san.as_deref(), Some("e4"));
    assert!(saved_tree.nodes.iter().all(|n| n.comment.is_none()));
}

#[tokio::test]
async fn a_global_seed_requires_admin() {
    let (svc, db_id) = setup().await;
    let (eval, stats) = opening_fixture();
    let tree = build_tree(&eval, &stats, STARTPOS_FEN, &tree_config(), STD)
        .await
        .unwrap();

    // alice is not an admin, so a global study is forbidden — nothing is persisted.
    let err = seed_study_from_tree(&svc, &alice(), &tree, &seed_params(db_id, true))
        .await
        .unwrap_err();
    assert!(matches!(err, StudyError::Forbidden));
    assert!(svc.list(&alice()).await.unwrap().is_empty());
}
