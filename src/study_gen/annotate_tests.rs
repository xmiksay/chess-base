//! Tests for [`super`] (LLM annotation pass + verification loop). Split out to
//! keep the module under the project's 500-line file cap. Trees are built by hand
//! so the verification loop runs against known eval / legality without an engine.

use super::*;
use crate::ai::llm::CompletionResponse;
use crate::engine::Score;
use crate::position::apply_san;
use crate::search::report::EcoInfo;
use crate::study_gen::features::Concepts;
use crate::study_gen::tree::VariationNode;

use async_trait::async_trait;
use std::sync::Mutex;

const STD: CastlingMode = CastlingMode::Standard;
const START: &str = "rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w KQkq - 0 1";

/// Build a bare variation node; callers tweak the fields they care about.
fn node(id: usize, parent: Option<usize>, san: Option<&str>, fen: &str) -> VariationNode {
    VariationNode {
        id,
        parent,
        san: san.map(Into::into),
        fen: fen.into(),
        zobrist: "0000000000000000".into(),
        ply: parent.map(|_| 1).unwrap_or(0),
        eval: None,
        stats: None,
        eco: None,
        concepts: Concepts::default(),
        shapes: Vec::new(),
        children: Vec::new(),
    }
}

/// Stub provider returning a fixed reply, recording the request it received so
/// the architecture test can inspect exactly what reached the model.
struct StubProvider {
    reply: String,
    last: Mutex<Option<CompletionRequest>>,
}

impl StubProvider {
    fn new(reply: impl Into<String>) -> Self {
        Self {
            reply: reply.into(),
            last: Mutex::new(None),
        }
    }
}

#[async_trait]
impl LlmProvider for StubProvider {
    async fn complete(&self, req: CompletionRequest) -> Result<CompletionResponse, ProviderError> {
        *self.last.lock().unwrap() = Some(req);
        Ok(CompletionResponse {
            text: Some(self.reply.clone()),
            tool_calls: Vec::new(),
            usage: None,
        })
    }
    fn name(&self) -> &'static str {
        "stub"
    }
    fn default_model(&self) -> &str {
        "stub-model"
    }
}

// ---------------------------------------------------------------------------
// Architecture test: the batch path puts no engine eval / PV in the context.
// ---------------------------------------------------------------------------

#[test]
fn prompt_excludes_engine_ground_truth() {
    // A node tagged with a very distinctive eval and DB count: neither may leak.
    let mut root = node(0, None, None, START);
    root.eval = Some(Score::Cp { value: 1234 });
    let after_e4 = "rnbqkbnr/pppppppp/8/8/4P3/8/PPPP1PPP/RNBQKBNR b KQkq - 0 1";
    let mut e4 = node(1, Some(0), Some("e4"), after_e4);
    e4.eval = Some(Score::Cp { value: -777 });
    e4.eco = Some(EcoInfo {
        eco: "B00".into(),
        name: "King's Pawn".into(),
    });
    e4.concepts.tags = vec!["open e-file".into()];
    root.children = vec![1];
    let tree = VariationTree {
        nodes: vec![root, e4],
        root: 0,
    };

    let req = build_request(&tree, "claude-opus-4-8");

    // Batch mode: no tools are offered to the model.
    assert!(req.tools.is_empty(), "batch path must expose no tools");

    // Everything that reaches the model: system + every message body.
    let mut ctx = req.system.clone();
    for m in &req.messages {
        if let Message::User { text } = m {
            ctx.push('\n');
            ctx.push_str(text);
        }
    }
    // No engine evaluations and no principal variation.
    assert!(!ctx.contains("1234"), "leaked a stored eval: {ctx}");
    assert!(!ctx.contains("777"), "leaked a stored eval: {ctx}");
    assert!(!ctx.to_lowercase().contains("pv"), "leaked a PV: {ctx}");
    // The teaching context the model *is* allowed: moves, opening, concepts.
    assert!(ctx.contains("e4"), "moves must reach the model");
    assert!(
        ctx.contains("King's Pawn"),
        "opening name should reach the model"
    );
    assert!(
        ctx.contains("open e-file"),
        "concepts should reach the model"
    );
}

// ---------------------------------------------------------------------------
// Acceptance: a deliberately wrong claim is caught and not committed.
// ---------------------------------------------------------------------------

/// Tree: 1.e4 is actually a fine move (eval barely changes for White), while a
/// hypothetical sibling blunder drops a queen's worth of eval.
fn wrong_claim_tree() -> VariationTree {
    let mut root = node(0, None, None, START);
    root.eval = Some(Score::Cp { value: 20 }); // White, +0.2 to move
    let after_e4 = "rnbqkbnr/pppppppp/8/8/4P3/8/PPPP1PPP/RNBQKBNR b KQkq - 0 1";
    let mut e4 = node(1, Some(0), Some("e4"), after_e4);
    // Black to move sees roughly equal: White's move kept the edge → not a loss.
    e4.eval = Some(Score::Cp { value: -10 });
    let mut na3 = node(2, Some(0), Some("Na3"), START); // fen unused for eval claims
    na3.eval = Some(Score::Cp { value: 600 }); // Black is +6 → White blundered
    root.children = vec![1, 2];
    VariationTree {
        nodes: vec![root, e4, na3],
        root: 0,
    }
}

#[tokio::test]
async fn wrong_claim_is_caught_and_not_committed() {
    let tree = wrong_claim_tree();
    // The model wrongly says 1.e4 loses a pawn, and rightly says Na3 is a blunder.
    let reply = r#"{"annotations":[
        {"node_id":1,"comment":"e4 drops a pawn here.","claims":[{"kind":"loses_material","pawns":1}]},
        {"node_id":2,"comment":"A terrible knight move.","claims":[{"kind":"blunder"}]}
    ]}"#;
    let provider = StubProvider::new(reply);
    let outcome = annotate_tree(&provider, &tree, "stub-model", STD)
        .await
        .unwrap();

    // The false claim was caught and the comment resting on it never committed.
    assert_eq!(
        outcome.tree.nodes[1].comment, None,
        "false comment must be dropped"
    );
    assert!(
        outcome
            .rejected
            .iter()
            .any(|r| r.node_id == 1 && r.what.contains("loses_material")),
        "the wrong claim must be recorded as rejected: {:?}",
        outcome.rejected
    );

    // The true claim survived and its comment committed.
    assert_eq!(
        outcome.tree.nodes[2].comment.as_deref(),
        Some("A terrible knight move."),
        "a verified comment must commit"
    );
}

#[tokio::test]
async fn annotate_tree_commits_a_clean_study() {
    let tree = wrong_claim_tree();
    // Plain prose with no concrete claims always commits; a verified question too.
    let reply = r#"{"annotations":[
        {"node_id":0,"comment":"The starting position.","question":"What does White aim for?"}
    ]}"#;
    let outcome = annotate_tree(&StubProvider::new(reply), &tree, "stub-model", STD)
        .await
        .unwrap();
    let root_comment = outcome.tree.nodes[0].comment.as_deref().unwrap();
    assert!(root_comment.contains("The starting position."));
    assert!(root_comment.contains("Training question: What does White aim for?"));
    assert!(outcome.rejected.is_empty());
}

// ---------------------------------------------------------------------------
// Verification-loop units.
// ---------------------------------------------------------------------------

#[test]
fn only_move_claim_checks_legality() {
    // Black is stalemated except for one pawn push: exactly one legal move.
    let forced = "7k/8/6Q1/8/8/p7/8/6K1 b - - 0 1";
    assert_eq!(
        legal_sans(forced, STD).unwrap().len(),
        1,
        "fixture must be forced"
    );
    let after = apply_san(forced, "a2", STD).unwrap().0;

    let root = node(0, None, None, forced);
    let mut a2 = node(1, Some(0), Some("a2"), &after);
    let mut root = root;
    root.children = vec![1];
    a2.eval = Some(Score::Cp { value: 0 });
    let tree = VariationTree {
        nodes: vec![root, a2],
        root: 0,
    };

    // Honest "only move" claim is confirmed.
    let drafts = vec![DraftAnnotation {
        node_id: 1,
        comment: Some("Forced.".into()),
        claims: vec![Claim::OnlyMove],
        ..Default::default()
    }];
    let out = verify_and_commit(&tree, &drafts, STD);
    assert_eq!(out.tree.nodes[1].comment.as_deref(), Some("Forced."));
    assert!(out.rejected.is_empty());
}

#[test]
fn only_move_claim_rejected_when_many_moves() {
    let mut root = node(0, None, None, START);
    let after_e4 = "rnbqkbnr/pppppppp/8/8/4P3/8/PPPP1PPP/RNBQKBNR b KQkq - 0 1";
    let e4 = node(1, Some(0), Some("e4"), after_e4);
    root.children = vec![1];
    let tree = VariationTree {
        nodes: vec![root, e4],
        root: 0,
    };
    let drafts = vec![DraftAnnotation {
        node_id: 1,
        comment: Some("The only move.".into()),
        claims: vec![Claim::OnlyMove],
        ..Default::default()
    }];
    let out = verify_and_commit(&tree, &drafts, STD);
    assert_eq!(out.tree.nodes[1].comment, None);
    assert_eq!(out.rejected.len(), 2, "claim + dropped comment recorded");
}

#[test]
fn nags_verified_independently() {
    // Sibling 1 is the best move (mover +30); sibling 2 is a blunder (mover -300).
    let mut root = node(0, None, None, START);
    root.eval = Some(Score::Cp { value: 20 });
    let mut good = node(1, Some(0), Some("e4"), START);
    good.eval = Some(Score::Cp { value: -30 }); // mover +30
    let mut bad = node(2, Some(0), Some("g4"), START);
    bad.eval = Some(Score::Cp { value: 300 }); // mover -300
    root.children = vec![1, 2];
    let tree = VariationTree {
        nodes: vec![root, good, bad],
        root: 0,
    };

    let drafts = vec![
        // `!` on the best move holds; `??` on it is contradicted and dropped.
        DraftAnnotation {
            node_id: 1,
            nags: vec![1, 4],
            ..Default::default()
        },
        // `??` on the real blunder holds; `!` on it is contradicted and dropped.
        DraftAnnotation {
            node_id: 2,
            nags: vec![4, 1],
            ..Default::default()
        },
    ];
    let out = verify_and_commit(&tree, &drafts, STD);
    assert_eq!(
        out.tree.nodes[1].nags,
        vec![1],
        "kept !, dropped ?? on best move"
    );
    assert_eq!(
        out.tree.nodes[2].nags,
        vec![4],
        "kept ??, dropped ! on blunder"
    );
    assert_eq!(out.rejected.len(), 2);
}

#[test]
fn wins_material_claim_confirmed_by_eval() {
    let mut root = node(0, None, None, START);
    root.eval = Some(Score::Cp { value: 0 });
    let mut grab = node(1, Some(0), Some("Nxe5"), START);
    grab.eval = Some(Score::Cp { value: -110 }); // mover +110 ≈ a pawn
    root.children = vec![1];
    let tree = VariationTree {
        nodes: vec![root, grab],
        root: 0,
    };
    let drafts = vec![DraftAnnotation {
        node_id: 1,
        comment: Some("Wins a pawn.".into()),
        claims: vec![Claim::WinsMaterial { pawns: 1 }],
        ..Default::default()
    }];
    let out = verify_and_commit(&tree, &drafts, STD);
    assert_eq!(out.tree.nodes[1].comment.as_deref(), Some("Wins a pawn."));
    assert!(out.rejected.is_empty());
}

#[test]
fn unknown_node_id_is_recorded_not_committed() {
    let tree = VariationTree {
        nodes: vec![node(0, None, None, START)],
        root: 0,
    };
    let drafts = vec![DraftAnnotation {
        node_id: 9,
        comment: Some("nowhere".into()),
        ..Default::default()
    }];
    let out = verify_and_commit(&tree, &drafts, STD);
    assert_eq!(out.tree.nodes.len(), 1);
    assert_eq!(out.rejected.len(), 1);
    assert_eq!(out.rejected[0].node_id, 9);
}

#[test]
fn move_tree_mirrors_variation_tree_shape() {
    let mut root = node(0, None, None, START);
    let after_e4 = "rnbqkbnr/pppppppp/8/8/4P3/8/PPPP1PPP/RNBQKBNR b KQkq - 0 1";
    let e4 = node(1, Some(0), Some("e4"), after_e4);
    root.children = vec![1];
    let tree = VariationTree {
        nodes: vec![root, e4],
        root: 0,
    };
    let out = verify_and_commit(&tree, &[], STD);
    assert_eq!(out.tree.mainline(), vec!["e4"]);
    assert_eq!(out.tree.root, 0);
}

#[test]
fn parses_json_wrapped_in_prose() {
    let text = "Sure! Here is the study:\n```json\n{\"annotations\":[{\"node_id\":0}]}\n```\nDone.";
    let drafts = parse_drafts(text).unwrap();
    assert_eq!(drafts.len(), 1);
    assert_eq!(drafts[0].node_id, 0);
}

#[test]
fn empty_response_text_is_an_error() {
    // A reply with no JSON object surfaces as a parse error.
    assert!(matches!(
        parse_drafts("no json here"),
        Err(AnnotateError::Parse(_))
    ));
}
