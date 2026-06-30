//! Tests for the danger-map study generator (#140). The spine walk is driven by
//! injected fakes — a multi-PV analyzer + a continuation source keyed by FEN — and
//! a stub LLM provider, while persistence runs against a real in-memory SQLite
//! study service. Split out to keep `danger_generate.rs` under the 500-line cap.

use super::*;

use std::collections::HashMap;
use std::sync::Mutex;

use anyhow::Result;
use async_trait::async_trait;
use sea_orm::{ActiveModelTrait, Set};

use crate::ai::llm::{CompletionRequest, CompletionResponse, LlmProvider, ProviderError};
use crate::db::entities::databases;
use crate::db::{connect, DbConfig};
use crate::engine::{Analysis, Score};
use crate::pgn_tree::MoveTree;
use crate::position::{replay, STARTPOS_FEN};
use crate::search::report::MoveReport;
use crate::study_gen::spine::Side;
use crate::study_gen::tree::ContinuationSource;
use crate::study_gen::MultiAnalyzer;

const STD: CastlingMode = CastlingMode::Standard;

// --- Fakes ----------------------------------------------------------------

struct FakeAnalyzer(HashMap<String, Vec<Analysis>>);
struct FakeStats(HashMap<String, Vec<MoveReport>>);

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

/// Stub LLM provider: returns a fixed reply (recording the request it received)
/// or fails, so the failure path is exercised too.
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

/// A White repertoire: 1.e4, prepared only against 1...c5.
fn white_spine() -> MoveTree {
    let mut t = MoveTree::new();
    let e4 = t.add_move(t.root, "e4");
    let c5 = t.add_move(e4, "c5");
    t.add_move(c5, "Nf3");
    t
}

/// After 1.e4 the engine sees Black's best (c5) hold only −10 (our downside
/// bounded), but the tempting e5 collapses to −300 (our baited upside): a weapon
/// trap tagged on e4. c5 is on-book; nothing else is offered.
fn weapon_fixture() -> (FakeAnalyzer, FakeStats) {
    let mut an = HashMap::new();
    an.insert(
        fen_after(&["e4"]),
        vec![line("c7c5", -10), line("e7e5", -300)],
    );
    let mut stats = HashMap::new();
    stats.insert(fen_after(&["e4"]), vec![report("c5", 0.5)]);
    (FakeAnalyzer(an), FakeStats(stats))
}

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

fn params(database_id: i32, start_fen: &str) -> DangerStudyParams {
    DangerStudyParams {
        database_id,
        name: "Danger".to_string(),
        global: false,
        start_fen: start_fen.to_string(),
        spine: white_spine(),
        spine_config: SpineConfig {
            our_side: Side::White,
            max_depth: 2,
            ..SpineConfig::default()
        },
        model: None,
    }
}

// --- Tests ----------------------------------------------------------------

#[tokio::test]
async fn generates_persists_and_surfaces_roles_and_rejections() {
    let (svc, db_id) = setup().await;
    let (an, stats) = weapon_fixture();
    // Node 0 (root): plain comment, always commits. Node 1 (e4): the weapon move —
    // an only_move claim that ground truth rejects (the start has 20 legal moves),
    // so its prose is dropped, exercising the verification loop against the danger
    // tree (which carries no per-node eval).
    let provider = StubProvider::replying(
        r#"{"annotations":[
            {"node_id":0,"comment":"The starting position."},
            {"node_id":1,"comment":"A nasty trap.","claims":[{"kind":"only_move"}]}
        ]}"#,
    );

    let outcome = generate_danger_study(
        &an,
        &stats,
        &provider,
        &svc,
        &alice(),
        &params(db_id, STARTPOS_FEN),
    )
    .await
    .expect("generation succeeds");

    // A study was saved, owned by the caller.
    assert_eq!(outcome.study.owner_id.as_deref(), Some("alice"));
    assert!(outcome.node_count >= 2); // root + e4 (+ c5)

    // The weapon role on e4 (node 1) is surfaced on the result.
    let weapon = outcome
        .roles
        .iter()
        .find(|r| r.san.as_deref() == Some("e4"))
        .expect("e4 carries a role");
    assert_eq!(weapon.node_id, 1);
    assert_eq!(weapon.kind, DangerKind::Trap);
    assert_eq!(weapon.role, DangerRole::Weapon);

    // The unverifiable only_move claim was rejected, never committed.
    assert!(outcome
        .rejected
        .iter()
        .any(|r| r.node_id == 1 && r.what.contains("only_move")));

    // The saved study carries the root comment but not the dropped one.
    let saved = svc.get(&alice(), outcome.study.id).await.unwrap();
    let tree: MoveTree = serde_json::from_str(&saved.tree_json).unwrap();
    assert_eq!(
        tree.nodes[0].comment.as_deref(),
        Some("The starting position.")
    );
    assert_eq!(tree.nodes[1].comment, None);
}

#[tokio::test]
async fn role_hint_reaches_the_model_prompt() {
    let (svc, db_id) = setup().await;
    let (an, stats) = weapon_fixture();
    let provider = StubProvider::replying(r#"{"annotations":[]}"#);

    generate_danger_study(
        &an,
        &stats,
        &provider,
        &svc,
        &alice(),
        &params(db_id, STARTPOS_FEN),
    )
    .await
    .unwrap();

    // The danger role rides into the prompt as a concept hint so the model
    // annotates danger-aware — but no engine number is anywhere in its context.
    let req = provider
        .last
        .lock()
        .unwrap()
        .take()
        .expect("model was called");
    let seen = format!("{} {:?}", req.system, req.messages);
    assert!(seen.contains("danger weapon"), "role hint missing: {seen}");
    assert!(!seen.contains("-300"), "engine eval leaked: {seen}");
}

#[tokio::test]
async fn invalid_start_fen_surfaces_a_clean_client_error() {
    let (svc, db_id) = setup().await;
    let (an, stats) = weapon_fixture();
    let provider = StubProvider::replying(r#"{"annotations":[]}"#);

    let err = generate_danger_study(
        &an,
        &stats,
        &provider,
        &svc,
        &alice(),
        &params(db_id, "not a fen"),
    )
    .await
    .unwrap_err();

    assert!(matches!(err, DangerStudyError::Spine(_)));
    assert_eq!(err.http_status_hint(), 400);
    assert!(err.client_message().contains("invalid FEN"));
}

#[tokio::test]
async fn llm_failure_surfaces_without_leaking_internals() {
    let (svc, db_id) = setup().await;
    let (an, stats) = weapon_fixture();
    let provider = StubProvider::failing();

    let err = generate_danger_study(
        &an,
        &stats,
        &provider,
        &svc,
        &alice(),
        &params(db_id, STARTPOS_FEN),
    )
    .await
    .unwrap_err();

    assert!(matches!(err, DangerStudyError::Annotate(_)));
    let msg = err.client_message();
    assert_eq!(msg, "the language model request failed");
    assert!(!msg.contains("stub network down"));
}

#[tokio::test]
async fn off_book_reply_is_tagged_and_persisted() {
    let (svc, db_id) = setup().await;
    // After 1.e4 the DB shows e5 (not prepared) more than c5: e5 becomes an
    // off-book leaf, c5 the on-book reply.
    let mut stats = HashMap::new();
    stats.insert(
        fen_after(&["e4"]),
        vec![report("c5", 0.5), report("e5", 0.4)],
    );
    let mut an = HashMap::new();
    an.insert(
        fen_after(&["e4"]),
        vec![line("c7c5", -10), line("e7e5", -20)],
    );
    let provider = StubProvider::replying(r#"{"annotations":[]}"#);

    let outcome = generate_danger_study(
        &FakeAnalyzer(an),
        &FakeStats(stats),
        &provider,
        &svc,
        &alice(),
        &params(db_id, STARTPOS_FEN),
    )
    .await
    .unwrap();

    let off_book = outcome
        .roles
        .iter()
        .find(|r| r.role == DangerRole::OffBook)
        .expect("an off-book role is surfaced");
    assert_eq!(off_book.san.as_deref(), Some("e5"));
    assert_eq!(off_book.kind, DangerKind::OffBook);
}
