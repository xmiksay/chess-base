// Graft an engine game review (issue #119, Mode A) onto a game's variation tree
// (issue #136). For every move the review flagged as critical — an inaccuracy,
// mistake or blunder — the engine's recommended continuation (`best_line`, #135)
// is appended as a *variation* off the position **before** the played move, so
// the better move becomes a sibling of what was actually played.
//
// Pure and framework-free over the `MoveTree` shape, so it is unit-testable
// without a board. It never touches `children[0]`, so the mainline — and the
// ply↔node mapping the games store derives from it — stays stable. Re-running it
// is idempotent: existing children are reused (via `childWithSan`) rather than
// duplicated, so re-analysing a game does not pile up copies of the same line.

import { appendChild, childWithSan, mainlinePath } from './moveTree'
import type { Eval, GameReview, MoveClassification, MoveReview, MoveTree } from '../types'

/** The buckets that earn a grafted engine line (mirrors src/review/classify.rs). */
const CRITICAL = new Set<MoveClassification>(['inaccuracy', 'mistake', 'blunder'])

/** The reviewed eval as a node `Eval` (White's perspective): mate wins over cp. */
function evalOf(mv: MoveReview): Eval {
  return mv.mate != null ? { mate: mv.mate } : { cp: mv.eval_cp }
}

/** Set a node's comment + eval immutably, returning the new tree. */
function annotate(tree: MoveTree, id: number, comment: string, ev: Eval): MoveTree {
  const nodes = tree.nodes.map((n) => (n.id === id ? { ...n, comment, eval: ev } : n))
  return { root: tree.root, nodes }
}

/**
 * Append `line` (a chain of SAN moves) as a continuation of `parentId`, reusing
 * any matching existing children so the graft is idempotent. Returns the new
 * tree and the id of the line's first node (the engine's better move), or null
 * if the line is empty.
 */
function graftLine(
  tree: MoveTree,
  parentId: number,
  line: string[],
): { tree: MoveTree; firstId: number } | null {
  let result = tree
  let parent = parentId
  let firstId: number | null = null
  for (const san of line) {
    let childId = childWithSan(result, parent, san)
    if (childId == null) {
      const appended = appendChild(result, parent, san)
      if (!appended) break
      result = appended.tree
      childId = appended.id
    }
    if (firstId == null) firstId = childId
    parent = childId
  }
  return firstId == null ? null : { tree: result, firstId }
}

/**
 * Return a copy of `tree` with the review's critical lines grafted in. The
 * mainline path is read once up front; because grafts only ever add later
 * children, those node ids stay valid for the whole pass.
 */
export function graftReviewVariations(tree: MoveTree, review: GameReview): MoveTree {
  let result = tree
  // index = ply, value = mainline node id (index 0 = root = start position).
  const mainline = mainlinePath(result)
  for (const mv of review.moves) {
    if (!CRITICAL.has(mv.classification)) continue
    const line = mv.best_line
    if (!line || line.length === 0) continue
    // The position before the played move (ply `mv.ply`) is the node one ply back.
    const priorId = mainline[mv.ply - 1]
    if (priorId == null) continue
    const grafted = graftLine(result, priorId, line)
    if (!grafted) continue
    result = annotate(grafted.tree, grafted.firstId, mv.explanation, evalOf(mv))
  }
  return result
}
