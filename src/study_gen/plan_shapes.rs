//! Pre-computed board arrows for generated studies: per node, the engine "plans"
//! (per-piece PV trajectories, ADR-0017) and the static "threats" scan
//! (hanging pieces, ADR-0024/#123), pinned onto the node as [`Shape`]s so they
//! persist with the study and render straight to the board.
//!
//! This is a deterministic preprocessing pass, not an LLM tool — the shapes are
//! engine/DB-grounded *data*, never fed to the model (ADR-0009). Pure helpers
//! ([`plan_to_shapes`] / [`node_shapes`]) are I/O-free and unit-tested; the only
//! I/O is the optional engine search behind the injected [`MultiAnalyzer`] seam.

use crate::pgn_tree::Shape;
use crate::plans::{plan_from_pv, Plan, DEFAULT_MAX_MOVES};
use crate::position::CastlingMode;
use crate::threats::threats;

use super::spine::MultiAnalyzer;
use super::tree::VariationTree;

/// Hard cap on plan lines: the frontend registers only `plan1`..`plan3` brushes
/// (`MAX_PLAN_LINES` in `lib/plansToShapes.ts`). Extra lines have no colour, so a
/// 4th+ would render with the default brush — clamp instead.
pub const MAX_PLAN_LINES: u8 = 3;

/// What to pin onto each node. Both off ⇒ the pass is a no-op.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ShapeConfig {
    /// Number of top engine PV lines to draw as plan arrows (0 = off, capped at
    /// [`MAX_PLAN_LINES`]).
    pub plan_lines: u8,
    /// Draw the static hanging-piece threat arrows.
    pub threats: bool,
}

impl ShapeConfig {
    /// Whether there is anything new to pin. Note this is **not** "nothing to
    /// do": regenerating shapes on an *existing* tree with an off config is a
    /// legitimate instruction to strip previously generated arrows (issue
    /// #191) — callers doing that must not use this to skip the pass.
    pub fn is_off(&self) -> bool {
        self.plan_lines == 0 && !self.threats
    }

    /// Whether the engine (plan lines) is needed; threats are pure and never are.
    fn needs_engine(&self) -> bool {
        self.plan_lines > 0
    }
}

/// Whether `brush` belongs to this pass's own arrows: the top-`MAX_PLAN_LINES`
/// plan brushes at full opacity (`plan1..plan3`), their dimmed frontend
/// live-overlay counterparts (`plan1d..plan3d`, `lib/plansToShapes.ts`), and the
/// static `threat` brush (`crate::threats`). Anything else is a shape the user
/// drew themselves. [`merge_shapes`] uses this to replace only what a
/// generation/analyse pass itself pinned (issue #191).
fn generated_brush(brush: &str) -> bool {
    matches!(
        brush,
        "plan1" | "plan2" | "plan3" | "plan1d" | "plan2d" | "plan3d" | "threat"
    )
}

/// Merge freshly generated shapes into a node's existing ones: every existing
/// shape whose brush [`generated_brush`] owns is dropped, every other
/// (user-drawn) shape survives, then `generated` is appended. Passing an empty
/// `generated` therefore strips a node's generated arrows outright — the
/// semantics a regenerate pass needs when a layer is off, PV-tracing failed, or
/// the node is terminal (issue #191): never leave a stale generated arrow
/// behind, never touch a shape the user drew.
pub fn merge_shapes(existing: &[Shape], generated: Vec<Shape>) -> Vec<Shape> {
    let mut merged: Vec<Shape> = existing
        .iter()
        .filter(|s| !generated_brush(&s.brush))
        .cloned()
        .collect();
    merged.extend(generated);
    merged
}

/// One plan's per-piece trajectories as chessground arrows: an arrow per
/// consecutive square pair of each piece path (`["g1","f3","g5"]` ⇒ `g1→f3`,
/// `f3→g5`), all under `brush`. Mirrors the frontend `lib/plansToShapes.ts`
/// segment mapping so a pinned plan looks like the live overlay.
pub fn plan_to_shapes(plan: &Plan, brush: &str) -> Vec<Shape> {
    let mut shapes = Vec::new();
    for traj in &plan.trajectories {
        for pair in traj.squares.windows(2) {
            let (orig, dest) = (&pair[0], &pair[1]);
            if orig == dest {
                continue;
            }
            shapes.push(Shape {
                orig: orig.clone(),
                dest: Some(dest.clone()),
                brush: brush.to_string(),
            });
        }
    }
    shapes
}

/// All arrows for one position: the top-`lines` plan trajectories (each line
/// `i` under brush `plan{i}`, capped at [`MAX_PLAN_LINES`]) drawn from `pvs`,
/// followed by the threat arrows when `want_threats`. `pvs` are UCI principal
/// variations best-first. Robust to bad data: a PV that cannot be traced and a
/// FEN that threats cannot parse contribute nothing rather than erroring.
pub fn node_shapes(
    fen: &str,
    pvs: &[Vec<String>],
    lines: u8,
    want_threats: bool,
    mode: CastlingMode,
) -> Vec<Shape> {
    let mut shapes = Vec::new();

    let take = (lines.min(MAX_PLAN_LINES)) as usize;
    for (rank, pv) in pvs.iter().take(take).enumerate() {
        if let Ok(plan) = plan_from_pv(fen, pv, DEFAULT_MAX_MOVES, mode) {
            let brush = format!("plan{}", rank + 1);
            shapes.extend(plan_to_shapes(&plan, &brush));
        }
    }

    if want_threats {
        if let Ok(threat_shapes) = threats(fen, mode) {
            shapes.extend(threat_shapes);
        }
    }

    shapes
}

/// Populate every node's `shapes` with its plan/threat arrows, merging via
/// [`merge_shapes`] so a node's existing shapes are replaced only in the
/// generated brushes (issue #191) — harmless here since a freshly built tree's
/// nodes start with no shapes at all, but this is the same pass a study
/// re-analyse reuses over an *existing* tree, where it matters. The engine is
/// queried (via `analyzer`) only when plan lines are requested; threats are a
/// pure scan over each node's FEN. Always walks every node — including when
/// `cfg.is_off()`, which now means "strip any generated arrows" rather than
/// "skip" — so callers regenerating an existing tree never need a separate
/// code path for turning a layer off. Per-node engine failures are skipped so
/// one bad position never aborts the whole study.
pub async fn apply_shapes(
    analyzer: Option<&(dyn MultiAnalyzer + Sync)>,
    tree: &mut VariationTree,
    cfg: &ShapeConfig,
    mode: CastlingMode,
) {
    let use_engine = cfg.needs_engine() && analyzer.is_some();

    for node in &mut tree.nodes {
        let pvs: Vec<Vec<String>> = if use_engine {
            match analyzer
                .expect("checked above")
                .analyse_multi(&node.fen)
                .await
            {
                Ok(lines) => lines.into_iter().map(|a| a.pv).collect(),
                Err(_) => Vec::new(),
            }
        } else {
            Vec::new()
        };
        let generated = node_shapes(&node.fen, &pvs, cfg.plan_lines, cfg.threats, mode);
        node.shapes = merge_shapes(&node.shapes, generated);
    }
}

#[cfg(test)]
#[path = "plan_shapes_tests.rs"]
mod tests;
