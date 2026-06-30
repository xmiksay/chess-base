//! LLM annotation pass + verification loop (issue #31): the final study-gen
//! stage. It turns a finished, tagged [`VariationTree`] (the tree builder #29 +
//! feature extractor #30) into an annotated [`MoveTree`] of comments, NAG glyphs
//! and training questions. Pure language work — the model never expands the tree.
//!
//! ## Batch mode: no tools, ground-truth verification (ADR-0009)
//!
//! In batch mode the model gets **no tools**: the engine and DB are internal
//! functions and their evaluations / principal variations are **never placed in
//! the model context**. [`build_prompt`] feeds the model only the moves, the
//! strategic concept tags (#30) and the opening name — see the architecture test
//! `prompt_excludes_engine_ground_truth`.
//!
//! The model's output is therefore a *draft*. Every concrete factual claim it
//! makes ("only move", "loses a pawn", "blunder", "best move") is attached as a
//! machine-checkable [`Claim`] and **verified against ground truth before
//! commit** ([`verify_and_commit`]): legal-move legality for "only move", and the
//! tree's stored engine evaluation (the engine *is* the ground truth that built
//! the tree) for the material / quality claims. A claim that the ground truth
//! contradicts — or cannot confirm — is dropped, and any prose that rested on it
//! is dropped with it, so a wrong claim never reaches the committed study.

use serde::{Deserialize, Serialize};

use crate::ai::llm::{CompletionRequest, LlmProvider, Message, ProviderError};
use crate::pgn_tree::{self, MoveTree};
use crate::position::{legal_sans, CastlingMode};

use super::tree::{score_to_cp, VariationTree};

/// One pawn of advantage, in the centipawn-equivalent units [`score_to_cp`]
/// produces. Material claims are confirmed against the stored engine eval, so
/// "loses a pawn" means the mover's evaluation drops by about this much.
const PAWN_CP: i32 = 100;
/// Slack allowed when confirming a material claim against the eval — the model
/// names whole pawns, the engine speaks in centipawns.
const MATERIAL_TOLERANCE_CP: i32 = 50;
/// Minimum eval drop (for the side that moved) that confirms a blunder / `??`.
const BLUNDER_CP: i32 = 200;
/// Minimum eval drop that confirms a mistake glyph (`?`).
const MISTAKE_CP: i32 = 100;

/// System prompt for the batch annotation pass. Deliberately mentions no
/// numbers: the model annotates from the moves and concepts alone and states its
/// factual claims so they can be checked. Avoids the words the architecture test
/// guards against ("eval", "pv") so the guard tests the *data*, not the prose.
const SYSTEM_PROMPT: &str = "\
You are a chess coach annotating a study tree. You are given a tree of moves with \
the strategic concepts present at each position and the opening name. Annotate it \
for a club player.

For each move you want to comment on, return one annotation object. Respond with a \
single JSON object and nothing else:

{\"annotations\": [\n  {\n    \"node_id\": <id from the tree>,\n    \
\"comment\": \"<prose, optional>\",\n    \"nags\": [<numeric glyphs, optional: 1=!, \
2=?, 3=!!, 4=??>],\n    \"question\": \"<a training question, optional>\",\n    \
\"claims\": [<concrete factual claims this move makes, optional>]\n  }\n]}

Every concrete factual statement in your prose MUST be backed by a claim object so \
it can be checked. Supported claims:
  {\"kind\":\"only_move\"}                 the move is the only legal move
  {\"kind\":\"best_move\"}                 the move is the strongest continuation
  {\"kind\":\"blunder\"}                   the move badly worsens the position
  {\"kind\":\"loses_material\",\"pawns\":N}  the move loses about N pawns
  {\"kind\":\"wins_material\",\"pawns\":N}   the move wins about N pawns

Do not invent facts. If you are unsure a claim holds, omit it.";

/// A concrete, machine-checkable factual claim the model attaches to a move.
/// Each variant is confirmed against ground truth — legal-move legality or the
/// tree's stored engine evaluation — with no second call to the model.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Claim {
    /// The move leading to this node is the only legal move (forced).
    OnlyMove,
    /// The move is the strongest continuation among the tree's siblings.
    BestMove,
    /// The move badly worsens the position for the side that moved.
    Blunder,
    /// The move loses roughly `pawns` pawns for the side that moved.
    LosesMaterial { pawns: u32 },
    /// The move wins roughly `pawns` pawns for the side that moved.
    WinsMaterial { pawns: u32 },
}

impl Claim {
    /// Short label for a [`Rejection`] record.
    fn label(&self) -> String {
        match self {
            Claim::OnlyMove => "claim only_move".into(),
            Claim::BestMove => "claim best_move".into(),
            Claim::Blunder => "claim blunder".into(),
            Claim::LosesMaterial { pawns } => format!("claim loses_material({pawns})"),
            Claim::WinsMaterial { pawns } => format!("claim wins_material({pawns})"),
        }
    }
}

/// One annotation the model proposes for a single tree node, before verification.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct DraftAnnotation {
    pub node_id: usize,
    #[serde(default)]
    pub comment: Option<String>,
    #[serde(default)]
    pub nags: Vec<u8>,
    #[serde(default)]
    pub question: Option<String>,
    #[serde(default)]
    pub claims: Vec<Claim>,
}

#[derive(Deserialize)]
struct DraftEnvelope {
    #[serde(default)]
    annotations: Vec<DraftAnnotation>,
}

/// Something the model proposed that ground truth rejected and that was therefore
/// not committed — recorded so the pipeline can report (or flag) it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rejection {
    pub node_id: usize,
    /// What was dropped, e.g. `"claim only_move"` or `"nag 4"`.
    pub what: String,
    /// Why ground truth rejected it.
    pub reason: String,
}

/// The annotated study plus everything the verification loop dropped.
#[derive(Clone, Debug)]
pub struct AnnotationOutcome {
    /// The committed study tree (comments + verified NAGs), mirroring the
    /// variation tree's shape.
    pub tree: MoveTree,
    /// Claims / glyphs ground truth rejected, never committed.
    pub rejected: Vec<Rejection>,
}

/// Why the annotation pass failed before it could verify anything.
#[derive(Debug, thiserror::Error)]
pub enum AnnotateError {
    #[error(transparent)]
    Provider(#[from] ProviderError),
    #[error("model returned no text")]
    EmptyResponse,
    #[error("parse model output: {0}")]
    Parse(String),
}

/// Run the batch annotation pass end to end: build the tool-free prompt, call the
/// model, parse the draft, then verify every claim against ground truth and
/// commit only what survives.
pub async fn annotate_tree(
    provider: &dyn LlmProvider,
    tree: &VariationTree,
    model: &str,
    castling: CastlingMode,
) -> Result<AnnotationOutcome, AnnotateError> {
    let resp = provider.complete(build_request(tree, model)).await?;
    let text = resp.text.ok_or(AnnotateError::EmptyResponse)?;
    let drafts = parse_drafts(&text)?;
    Ok(verify_and_commit(tree, &drafts, castling))
}

/// Build the batch completion request: the tool-free prompt under the annotation
/// system prompt. **No tools** — the engine/DB are not exposed to the model.
pub fn build_request(tree: &VariationTree, model: &str) -> CompletionRequest {
    CompletionRequest::new(model, vec![Message::user(build_prompt(tree))])
        .with_system(SYSTEM_PROMPT)
}

/// Render the tree as the model's context: per node its id, the line of moves
/// leading to it, the strategic concept tags (#30) and the opening name. Carries
/// **no** engine evaluation, principal variation or DB statistics — those are
/// ground truth used only for verification, never shown to the model (ADR-0009).
pub fn build_prompt(tree: &VariationTree) -> String {
    let mut out = String::from("Move tree to annotate:\n");
    for node in &tree.nodes {
        let line = line_to(tree, node.id);
        if line.is_empty() {
            out.push_str(&format!("- node {}: starting position", node.id));
        } else {
            out.push_str(&format!("- node {}: {}", node.id, line.join(" ")));
        }
        if let Some(eco) = &node.eco {
            out.push_str(&format!(" [{} {}]", eco.eco, eco.name));
        }
        if !node.concepts.tags.is_empty() {
            out.push_str(&format!(" — concepts: {}", node.concepts.tags.join("; ")));
        }
        out.push('\n');
    }
    out
}

/// Verify the drafted annotations against ground truth and commit the survivors
/// into a fresh [`MoveTree`] mirroring the variation tree's shape. Pure: it reads
/// only the tree's stored eval and the rules of chess (legal moves), so the whole
/// loop is unit-tested without an engine.
///
/// A node's prose (comment + question) is committed only if **every** concrete
/// claim it made was confirmed; otherwise the prose is dropped along with the
/// claim, so a false statement can never reach the study. NAG glyphs are verified
/// independently, glyph by glyph.
pub fn verify_and_commit(
    tree: &VariationTree,
    drafts: &[DraftAnnotation],
    castling: CastlingMode,
) -> AnnotationOutcome {
    let mut out = move_tree_from(tree);
    let mut rejected = Vec::new();

    for draft in drafts {
        if tree.nodes.get(draft.node_id).is_none() {
            rejected.push(Rejection {
                node_id: draft.node_id,
                what: "annotation".into(),
                reason: "no such node in the tree".into(),
            });
            continue;
        }

        // Verify every concrete claim; the prose stands only if all of them hold.
        let mut claims_ok = true;
        for claim in &draft.claims {
            if let Err(reason) = verify_claim(tree, draft.node_id, claim, castling) {
                claims_ok = false;
                rejected.push(Rejection {
                    node_id: draft.node_id,
                    what: claim.label(),
                    reason,
                });
            }
        }

        if claims_ok {
            if let Some(text) = commit_text(draft) {
                out.set_comment(draft.node_id, text);
            }
        } else if draft.comment.is_some() || draft.question.is_some() {
            rejected.push(Rejection {
                node_id: draft.node_id,
                what: "comment".into(),
                reason: "dropped: it rests on a claim ground truth rejected".into(),
            });
        }

        // NAG glyphs are independent assertions — verify each on its own.
        for &nag in &draft.nags {
            match verify_nag(tree, draft.node_id, nag) {
                Ok(()) => out.add_nag(draft.node_id, nag),
                Err(reason) => rejected.push(Rejection {
                    node_id: draft.node_id,
                    what: format!("nag {nag}"),
                    reason,
                }),
            }
        }
    }

    AnnotationOutcome {
        tree: out,
        rejected,
    }
}

/// The committed comment text: the prose with the training question appended, or
/// `None` if the draft carried neither.
fn commit_text(draft: &DraftAnnotation) -> Option<String> {
    let mut text = draft.comment.clone().unwrap_or_default();
    if let Some(q) = &draft.question {
        if !text.is_empty() {
            text.push_str("\n\n");
        }
        text.push_str("Training question: ");
        text.push_str(q);
    }
    (!text.is_empty()).then_some(text)
}

/// Confirm one concrete claim against ground truth, or return why it failed.
fn verify_claim(
    tree: &VariationTree,
    node_id: usize,
    claim: &Claim,
    castling: CastlingMode,
) -> Result<(), String> {
    match claim {
        Claim::OnlyMove => {
            let parent_fen =
                parent_fen(tree, node_id).ok_or_else(|| "the root has no move".to_string())?;
            let n = legal_sans(parent_fen, castling)
                .map_err(|e| format!("cannot read legal moves: {e}"))?
                .len();
            if n == 1 {
                Ok(())
            } else {
                Err(format!("the position has {n} legal moves, not one"))
            }
        }
        Claim::BestMove => match is_best_sibling(tree, node_id) {
            Some(true) => Ok(()),
            Some(false) => Err("a sibling move is evaluated higher".into()),
            None => Err("no engine evaluation to confirm against".into()),
        },
        Claim::Blunder => {
            let delta = mover_eval_delta(tree, node_id)
                .ok_or_else(|| "no engine evaluation to confirm against".to_string())?;
            if delta <= -BLUNDER_CP {
                Ok(())
            } else {
                Err(format!("the move changes the evaluation by {delta}cp"))
            }
        }
        Claim::LosesMaterial { pawns } => {
            let delta = mover_eval_delta(tree, node_id)
                .ok_or_else(|| "no engine evaluation to confirm against".to_string())?;
            let needed = (*pawns).max(1) as i32 * PAWN_CP - MATERIAL_TOLERANCE_CP;
            if delta <= -needed {
                Ok(())
            } else {
                Err(format!("the move changes the evaluation by {delta}cp"))
            }
        }
        Claim::WinsMaterial { pawns } => {
            let delta = mover_eval_delta(tree, node_id)
                .ok_or_else(|| "no engine evaluation to confirm against".to_string())?;
            let needed = (*pawns).max(1) as i32 * PAWN_CP - MATERIAL_TOLERANCE_CP;
            if delta >= needed {
                Ok(())
            } else {
                Err(format!("the move changes the evaluation by {delta}cp"))
            }
        }
    }
}

/// Verify a NAG glyph. Only the move-quality glyphs carry a concrete assertion we
/// can check — `!`/`!!` claim the best move, `?`/`??` claim a worsening move.
/// Every other glyph is descriptive and passes through unverified.
fn verify_nag(tree: &VariationTree, node_id: usize, nag: u8) -> Result<(), String> {
    match nag {
        1 | 3 => match is_best_sibling(tree, node_id) {
            Some(true) => Ok(()),
            Some(false) => Err("a sibling move is evaluated higher".into()),
            None => Err("no engine evaluation to confirm against".into()),
        },
        2 | 4 => {
            let delta = mover_eval_delta(tree, node_id)
                .ok_or_else(|| "no engine evaluation to confirm against".to_string())?;
            if delta <= -MISTAKE_CP {
                Ok(())
            } else {
                Err(format!("the move changes the evaluation by {delta}cp"))
            }
        }
        _ => Ok(()),
    }
}

/// SAN moves from the root down to `id` (inclusive); empty for the root.
fn line_to(tree: &VariationTree, id: usize) -> Vec<String> {
    let mut sans = Vec::new();
    let mut cur = Some(id);
    while let Some(i) = cur {
        let node = &tree.nodes[i];
        if let Some(san) = &node.san {
            sans.push(san.clone());
        }
        cur = node.parent;
    }
    sans.reverse();
    sans
}

/// The FEN of the position *before* the move into `node_id` (its parent's FEN).
fn parent_fen(tree: &VariationTree, node_id: usize) -> Option<&str> {
    let parent = tree.nodes[node_id].parent?;
    Some(tree.nodes[parent].fen.as_str())
}

/// Change in the position's evaluation, in centipawns and from the perspective of
/// the side that made the move into `node_id`: positive means the move improved
/// its position, negative means it worsened it. `None` unless both the node and
/// its parent carry an engine evaluation.
fn mover_eval_delta(tree: &VariationTree, node_id: usize) -> Option<i32> {
    let node = tree.nodes.get(node_id)?;
    let parent = &tree.nodes[node.parent?];
    parent.eval?;
    node.eval?;
    // Parent eval is from the mover's perspective (they are to move there); the
    // node eval is from the opponent's, so negate it to compare like with like.
    let before = score_to_cp(parent.eval);
    let after = -score_to_cp(node.eval);
    Some(after - before)
}

/// Whether `node_id`'s move is (tied) best among its siblings by stored eval, all
/// scored from the mover's perspective. `None` if the node carries no eval.
fn is_best_sibling(tree: &VariationTree, node_id: usize) -> Option<bool> {
    let node = tree.nodes.get(node_id)?;
    node.eval?;
    let siblings = &tree.nodes[node.parent?].children;
    let mover_eval = |id: usize| -score_to_cp(tree.nodes[id].eval);
    let best = siblings.iter().map(|&s| mover_eval(s)).max()?;
    Some(mover_eval(node_id) >= best)
}

/// A fresh [`MoveTree`] with the same shape (ids, moves, children) as the
/// variation tree — both are root-0 arenas, so the mapping is 1:1. Shared by the
/// annotation commit and the LLM-free seed path (issue #155).
pub fn move_tree_from(tree: &VariationTree) -> MoveTree {
    let nodes = tree
        .nodes
        .iter()
        .map(|v| pgn_tree::Node {
            id: v.id,
            parent: v.parent,
            san: v.san.clone(),
            comment: None,
            nags: Vec::new(),
            shapes: Vec::new(),
            eval: None,
            children: v.children.clone(),
        })
        .collect();
    // Carry a set-up origin through: a generated study can start from a custom
    // FEN, so the persisted tree records it (unless it is the standard start).
    let root_fen = &tree.nodes[tree.root].fen;
    MoveTree {
        nodes,
        root: tree.root,
        start_fen: (root_fen != crate::position::STARTPOS_FEN).then(|| root_fen.clone()),
    }
}

/// Parse the model's reply into draft annotations, tolerating prose around the
/// JSON object (the model may wrap it in a fence or a sentence).
fn parse_drafts(text: &str) -> Result<Vec<DraftAnnotation>, AnnotateError> {
    let json = extract_json_object(text)
        .ok_or_else(|| AnnotateError::Parse("no JSON object in the response".into()))?;
    let env: DraftEnvelope =
        serde_json::from_str(json).map_err(|e| AnnotateError::Parse(e.to_string()))?;
    Ok(env.annotations)
}

/// The outermost `{ … }` span in `text`, if any.
fn extract_json_object(text: &str) -> Option<&str> {
    let start = text.find('{')?;
    let end = text.rfind('}')?;
    (end >= start).then(|| &text[start..=end])
}

#[cfg(test)]
#[path = "annotate_tests.rs"]
mod tests;
