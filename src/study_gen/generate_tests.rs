//! Tests for the study-generation orchestrator (#115). The three stages are
//! driven by injected fakes — a fake evaluator + continuation source (so no
//! engine/DB is needed) and a stub LLM provider — while persistence runs against
//! a real in-memory SQLite study service. Split out to keep `generate.rs` under
//! the project's 500-line file cap.

use super::*;

use std::collections::HashMap;
use std::sync::Mutex;

use anyhow::Result;
use async_trait::async_trait;
use sea_orm::{ActiveModelTrait, Set};

use crate::ai::llm::{CompletionRequest, CompletionResponse, LlmProvider, ProviderError};
use crate::db::entities::databases;
use crate::db::{connect, DbConfig};
use crate::engine::Score;
use crate::pgn_tree::MoveTree;
use crate::position::{replay, STARTPOS_FEN};
use crate::search::report::MoveReport;
use crate::study_gen::tree::{ContinuationSource, Evaluator};

const STD: CastlingMode = CastlingMode::Standard;

// --- Fakes ----------------------------------------------------------------

struct FakeEval(HashMap<String, Score>);
struct FakeStats(HashMap<String, Vec<MoveReport>>);

#[async_trait]
impl Evaluator for FakeEval {
    async fn eval(&self, fen: &str) -> Result<Option<Score>> {
        Ok(self.0.get(fen).copied())
    }
}

#[async_trait]
impl ContinuationSource for FakeStats {
    async fn continuations(&self, fen: &str) -> Result<Vec<MoveReport>> {
        Ok(self.0.get(fen).cloned().unwrap_or_default())
    }
}

/// Stub LLM provider: either returns a fixed reply (recording the request it
/// received) or fails with a provider error, so failure paths are exercised too.
struct StubProvider {
    reply: Result<String, ()>,
    last: Mutex<Option<CompletionRequest>>,
}

impl StubProvider {
    fn replying(text: impl Into<String>) -> Self {
        Self {
            reply: Ok(text.into()),
            last: Mutex::new(None),
        }
    }

    fn failing() -> Self {
        Self {
            reply: Err(()),
            last: Mutex::new(None),
        }
    }
}

#[async_trait]
impl LlmProvider for StubProvider {
    async fn complete(&self, req: CompletionRequest) -> Result<CompletionResponse, ProviderError> {
        *self.last.lock().unwrap() = Some(req);
        match &self.reply {
            Ok(text) => Ok(CompletionResponse {
                text: Some(text.clone()),
                tool_calls: Vec::new(),
                usage: None,
            }),
            Err(()) => Err(ProviderError::Transport("stub network down".into())),
        }
    }
    fn name(&self) -> &'static str {
        "stub"
    }
    fn default_model(&self) -> &str {
        "stub-model"
    }
}

// --- Fixtures -------------------------------------------------------------

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

/// From the start: e4 (common) and d4 (common); after e4, only c5. The root eval
/// is a distinctive number used to assert it never reaches the model context.
fn fixture() -> (FakeEval, FakeStats) {
    let mut conts = HashMap::new();
    conts.insert(fen_after(&[]), vec![report("e4", 0.6), report("d4", 0.3)]);
    conts.insert(fen_after(&["e4"]), vec![report("c5", 0.7)]);

    let mut evals = HashMap::new();
    evals.insert(fen_after(&[]), Score::Cp { value: 1234 });
    evals.insert(fen_after(&["e4"]), Score::Cp { value: -30 });
    evals.insert(fen_after(&["d4"]), Score::Cp { value: 40 });
    evals.insert(fen_after(&["e4", "c5"]), Score::Cp { value: 25 });
    (FakeEval(evals), FakeStats(conts))
}

/// Fresh in-memory DB seeded with one database row owned by `alice`; returns the
/// service and that database's id.
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

fn params(database_id: i32, start_fen: &str) -> GenerateParams {
    GenerateParams {
        database_id,
        name: "Generated".to_string(),
        global: false,
        start_fen: start_fen.to_string(),
        tree: TreeConfig {
            max_depth: 2,
            max_children: 5,
            max_nodes: 100,
            min_frequency: 0.0,
            eval_margin_cp: 10_000,
            ..TreeConfig::default()
        },
        model: None,
    }
}

// --- Tests ----------------------------------------------------------------

#[tokio::test]
async fn generates_persists_and_annotates_a_study() {
    let (svc, db_id) = setup().await;
    let (eval, stats) = fixture();
    // Node 0 is always the root; commit a comment there (no claims ⇒ always
    // commits). Node 1 (e4) carries an only_move claim that ground truth rejects,
    // so its prose is dropped — exercising the verification loop.
    let provider = StubProvider::replying(
        r#"{"annotations":[
            {"node_id":0,"comment":"The starting position."},
            {"node_id":1,"comment":"Forced.","claims":[{"kind":"only_move"}]}
        ]}"#,
    );

    let outcome = generate_study(
        &eval,
        &stats,
        &provider,
        &svc,
        &alice(),
        &params(db_id, STARTPOS_FEN),
    )
    .await
    .expect("generation succeeds");

    // A study was saved, owned by the caller, with the full tree.
    assert_eq!(outcome.study.owner_id.as_deref(), Some("alice"));
    assert!(outcome.node_count >= 3); // root + e4 + d4 (+ c5)

    // The rejected only_move claim was dropped, never committed.
    assert!(outcome
        .rejected
        .iter()
        .any(|r| r.node_id == 1 && r.what.contains("only_move")));

    // The saved study is visible via the normal read path and carries the
    // committed annotation; the rejected node's prose did not survive.
    let saved = svc.get(&alice(), outcome.study.id).await.unwrap();
    let tree: MoveTree = serde_json::from_str(&saved.tree_json).unwrap();
    assert_eq!(
        tree.nodes[0].comment.as_deref(),
        Some("The starting position.")
    );
    assert_eq!(tree.nodes[1].comment, None);

    // It also shows up in the caller's study list.
    let listed = svc.list(&alice()).await.unwrap();
    assert!(listed.iter().any(|s| s.id == outcome.study.id));
}

#[tokio::test]
async fn batch_invariant_keeps_engine_eval_out_of_the_model_context() {
    let (svc, db_id) = setup().await;
    let (eval, stats) = fixture();
    let provider = StubProvider::replying(r#"{"annotations":[]}"#);

    generate_study(
        &eval,
        &stats,
        &provider,
        &svc,
        &alice(),
        &params(db_id, STARTPOS_FEN),
    )
    .await
    .unwrap();

    // The distinctive root eval (1234) must not appear anywhere the model saw.
    let req = provider
        .last
        .lock()
        .unwrap()
        .take()
        .expect("model was called");
    let seen = format!("{} {:?}", req.system, req.messages);
    assert!(
        !seen.contains("1234"),
        "engine eval leaked into the prompt: {seen}"
    );
}

#[tokio::test]
async fn invalid_start_fen_surfaces_a_clean_client_error() {
    let (svc, db_id) = setup().await;
    let (eval, stats) = fixture();
    let provider = StubProvider::replying(r#"{"annotations":[]}"#);

    let err = generate_study(
        &eval,
        &stats,
        &provider,
        &svc,
        &alice(),
        &params(db_id, "not a fen"),
    )
    .await
    .unwrap_err();

    assert!(matches!(err, GenerateError::Tree(_)));
    assert_eq!(err.http_status_hint(), 400);
    assert!(err.client_message().contains("invalid FEN"));
}

#[tokio::test]
async fn llm_failure_surfaces_without_leaking_internals() {
    let (svc, db_id) = setup().await;
    let (eval, stats) = fixture();
    let provider = StubProvider::failing();

    let err = generate_study(
        &eval,
        &stats,
        &provider,
        &svc,
        &alice(),
        &params(db_id, STARTPOS_FEN),
    )
    .await
    .unwrap_err();

    assert!(matches!(err, GenerateError::Annotate(_)));
    let msg = err.client_message();
    assert_eq!(msg, "the language model request failed");
    // The raw transport detail must not leak to the client.
    assert!(!msg.contains("stub network down"));
}

#[tokio::test]
async fn empty_db_position_yields_a_trivial_single_node_study() {
    let (svc, db_id) = setup().await;
    // No continuations and no eval anywhere ⇒ the tree is just the root.
    let eval = FakeEval(HashMap::new());
    let stats = FakeStats(HashMap::new());
    let provider = StubProvider::replying(r#"{"annotations":[]}"#);

    let outcome = generate_study(
        &eval,
        &stats,
        &provider,
        &svc,
        &alice(),
        &params(db_id, STARTPOS_FEN),
    )
    .await
    .expect("a bare position still yields a (single-node) study");

    assert_eq!(outcome.node_count, 1);
    let saved = svc.get(&alice(), outcome.study.id).await.unwrap();
    let tree: MoveTree = serde_json::from_str(&saved.tree_json).unwrap();
    assert_eq!(tree.nodes.len(), 1);
}
