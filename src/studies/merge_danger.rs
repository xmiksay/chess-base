//! Graft an engine-walked danger tree into a study, carrying the role verdict
//! onto each freshly grafted node (issue #177, completing the "possible future
//! extension" ADR-0032 called out): a White-perspective `[%eval]`, a short role
//! comment quoting the verdict's own figures, and a `!`/`?!` NAG so a Weapon or
//! Caution reads at a glance without opening the comment. **Never** touches a
//! node the graft only followed (an existing line) — the same non-destructive
//! contract issue #162's `analyse_study` uses for eval.
//!
//! Split out of `studies/mod.rs` to keep that file under the project's line cap
//! (the same reason `add_line.rs` / `mark_transpositions.rs` are their own files).

use crate::db::entities::studies;
use crate::pgn_tree::{Eval, MoveTree};
use crate::server::identity::CurrentUser;
use crate::study_gen::annotate::move_tree_from;
use crate::study_gen::danger_generate::to_variation_tree;
use crate::study_gen::{DangerKind, DangerRole, DangerTag, DangerTree, TrapVerdict};

use super::{StudyError, StudyService};

/// `$1` — a strong move (`!`), pinned on a grafted Weapon.
const NAG_GOOD_MOVE: u8 = 1;
/// `$6` — a dubious move (`?!`), pinned on a grafted Caution.
const NAG_DUBIOUS_MOVE: u8 = 6;

/// The refreshed study plus what the graft actually added, so a caller (the FE
/// "Extend this study" button) can report something better than a blind
/// reload — "14 new nodes, 3 Weapons, 2 Cautions" — or "no new lines" on an
/// idempotent re-merge.
#[derive(Debug, Clone)]
pub struct MergeDangerOutcome {
    pub study: studies::Model,
    /// Nodes the graft created; a node it only followed (already in the study)
    /// is not counted.
    pub added_nodes: usize,
    /// Of the added nodes, how many carried a Weapon role.
    pub weapons: usize,
    /// Of the added nodes, how many carried a Caution role.
    pub cautions: usize,
}

impl StudyService {
    /// Graft an engine-walked [`DangerTree`] into a study the caller may write,
    /// as deduped variations under `at_node_id` (defaults to the root). The
    /// danger tree is folded into a
    /// [`VariationTree`](crate::study_gen::tree::VariationTree) → a [`MoveTree`]
    /// (the same fold the danger-map generator uses), then grafted via
    /// [`MoveTree::graft_subtree`] so existing lines are followed (no
    /// duplicates) and new ones appended. Every node the graft actually
    /// **creates** (never one it only follows) is annotated from its
    /// [`DangerTag`]: `[%eval]`, a short role comment, and a `!`/`?!` NAG for a
    /// Weapon/Caution.
    pub async fn merge_danger(
        &self,
        user: &CurrentUser,
        study_id: i32,
        danger: DangerTree,
        at_node_id: Option<usize>,
    ) -> Result<MergeDangerOutcome, StudyError> {
        let study = self.load_writable(user, study_id).await?;
        let mut tree: MoveTree = serde_json::from_str(&study.tree_json)?;
        let at = at_node_id.unwrap_or(tree.root);
        if tree.line_to(at).is_none() {
            return Err(StudyError::InvalidNode(at));
        }
        let src = move_tree_from(&to_variation_tree(&danger));
        let added = tree.graft_subtree(at, &src);

        let mut weapons = 0;
        let mut cautions = 0;
        for (src_id, dst_id) in &added {
            let Some(tag) = danger.nodes.get(*src_id).and_then(|n| n.tag.as_ref()) else {
                continue;
            };
            annotate_grafted_node(&mut tree, *dst_id, tag);
            match tag.role {
                DangerRole::Weapon => weapons += 1,
                DangerRole::Caution => cautions += 1,
                DangerRole::OffBook => {}
            }
        }

        self.persist(study, &tree).await?;
        let study = self.get(user, study_id).await?;
        Ok(MergeDangerOutcome {
            study,
            added_nodes: added.len(),
            weapons,
            cautions,
        })
    }
}

/// Annotate one freshly grafted node from its danger tag: pin the eval (when
/// the position was searched), set a short role comment, and add a `!`
/// (Weapon) / `?!` (Caution) NAG. Off-book nodes get the comment but no eval
/// (never searched) and no move-quality NAG — missing an answer isn't a move
/// quality verdict. Every call here targets a node the graft just created, so
/// there is nothing to clobber.
fn annotate_grafted_node(tree: &mut MoveTree, node_id: usize, tag: &DangerTag) {
    if let Some(eval) = tag.eval {
        tree.set_eval(node_id, eval);
    }
    tree.set_comment(node_id, danger_comment(tag));
    match tag.role {
        DangerRole::Weapon => tree.add_nag(node_id, NAG_GOOD_MOVE),
        DangerRole::Caution => tree.add_nag(node_id, NAG_DUBIOUS_MOVE),
        DangerRole::OffBook => {}
    }
}

/// A short, human-readable summary of the role verdict, quoting the figures the
/// walk already computed, e.g. `"Weapon: trap, bounded downside on the best
/// reply (+0.30)"` or `"Caution: only move, 42% miss rate"`. Switches on
/// [`DangerTag::kind`] — the classifier's own priority order (Trap > OnlyMove >
/// Attack) already decided which verdict won, so this must not re-derive it
/// from which supplementary figures happen to be populated (`only_move_gap` is
/// computed unconditionally and can be `Some` even on a `Trap`/`Attack` tag).
fn danger_comment(tag: &DangerTag) -> String {
    let role = match tag.role {
        DangerRole::Weapon => "Weapon",
        DangerRole::Caution => "Caution",
        DangerRole::OffBook => "Off-book",
    };
    let detail = match tag.kind {
        DangerKind::Trap => match tag.trap {
            Some(TrapVerdict::Weapon) => "trap, bounded downside on the best reply".to_string(),
            Some(TrapVerdict::HopeChess) => "baited trap, the best reply refutes it".to_string(),
            _ => "trap".to_string(),
        },
        DangerKind::OnlyMove => {
            let miss = tag
                .miss_rate
                .map(|m| format!(", {:.0}% miss rate", m * 100.0))
                .unwrap_or_default();
            format!("only move{miss}")
        }
        DangerKind::Attack => "pawn storm toward your king".to_string(),
        DangerKind::OffBook => "no prepared answer in this repertoire".to_string(),
    };
    match tag.eval {
        Some(eval) => format!("{role}: {detail} ({})", format_eval(eval)),
        None => format!("{role}: {detail}"),
    }
}

/// Format an [`Eval`] as a signed pawn score (`+0.30`, `-0.40`) or a mate count
/// (`M3`, `-M2`), matching how a human reads a `[%eval]`.
fn format_eval(eval: Eval) -> String {
    match eval {
        Eval::Cp(cp) => format!("{:+.2}", cp as f64 / 100.0),
        Eval::Mate(n) if n >= 0 => format!("M{n}"),
        Eval::Mate(n) => format!("-M{}", -n),
    }
}

#[cfg(test)]
#[path = "merge_danger_tests.rs"]
mod tests;
