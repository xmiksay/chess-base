//! Regenerate plan/threat arrows on an existing study (issue #191, ADR-0039
//! addendum): re-run the `study_gen` shapes pass over a persisted [`MoveTree`],
//! merging into each node's shapes so user-drawn annotations survive and stale
//! generated arrows never linger — including when the caller turns every layer
//! off, which is itself the "clear the generated arrows" instruction.
//!
//! Kept out of `mod.rs` (already over the file-size cap), mirroring
//! `mark_transpositions.rs`. The engine walk costs one call per node when plan
//! lines are requested (mirrors `study_gen::generate`'s own pass over a fresh
//! tree) — it does not reuse `analyse_study`'s classification search, which is
//! MultiPV=2 at the position *before* a move and so is the wrong PV source for
//! a node's own plan arrows (those trace *from* the node's position).

use crate::db::entities::studies;
use crate::pgn_tree::MoveTree;
use crate::server::identity::CurrentUser;
use crate::study_gen::plan_shapes::{merge_shapes, node_shapes, ShapeConfig};
use crate::study_gen::spine::MultiAnalyzer;

use super::analyse::node_searches;
use super::{StudyError, StudyService, MODE};

impl StudyService {
    /// Regenerate every node's plan/threat arrows from `cfg` on a study the
    /// caller may write, merging into each node's existing shapes
    /// ([`merge_shapes`]) and returning the refreshed study. `analyzer` is
    /// queried only when `cfg.plan_lines > 0`; pass `None` when only `threats`
    /// is wanted, or to strip a study's generated arrows with `cfg.is_off()`
    /// and no engine at all.
    pub async fn regenerate_shapes(
        &self,
        analyzer: Option<&(dyn MultiAnalyzer + Sync)>,
        user: &CurrentUser,
        id: i32,
        cfg: &ShapeConfig,
    ) -> Result<studies::Model, StudyError> {
        let study = self.load_writable(user, id).await?;
        let mut tree: MoveTree = serde_json::from_str(&study.tree_json)?;

        let use_engine = cfg.plan_lines > 0 && analyzer.is_some();

        // Every node's own position: the root's start position, plus each
        // move-bearing node's after-the-move FEN (`node_searches` already skips
        // the root, which carries no move).
        let mut fens: Vec<(usize, String)> = vec![(tree.root, tree.start_position().to_string())];
        fens.extend(
            node_searches(&tree)?
                .into_iter()
                .map(|s| (s.node_id, s.fen_after)),
        );

        for (node_id, fen) in fens {
            let pvs: Vec<Vec<String>> = if use_engine {
                match analyzer.expect("checked above").analyse_multi(&fen).await {
                    Ok(lines) => lines.into_iter().map(|a| a.pv).collect(),
                    Err(_) => Vec::new(),
                }
            } else {
                Vec::new()
            };
            let generated = node_shapes(&fen, &pvs, cfg.plan_lines, cfg.threats, MODE);
            let merged = merge_shapes(&tree.nodes[node_id].shapes, generated);
            tree.set_shapes(node_id, merged);
        }

        self.persist(study, &tree).await?;
        self.get(user, id).await
    }
}

#[cfg(test)]
#[path = "regen_shapes_tests.rs"]
mod tests;
