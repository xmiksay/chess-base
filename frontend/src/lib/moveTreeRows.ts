// Flatten a `MoveTree` into rows for the notation panel.
//
// Replaces the token-stream + nested-block model (`treeTokens` / `tokenBlocks`).
// Two reasons it is a flat array rather than a nested item tree:
//
//  * Re-render cost. The panel keys a `v-for` on rows and each row carries a
//    `v-memo`, so stepping the cursor re-renders exactly the two rows that
//    changed. The nested renderer re-walked the whole tree on every ply.
//  * Depth. A row knows its own nesting depth as a number, so the view can
//    scale indent/colour/weight by it. The old renderer only knew "mainline or
//    not", which made a depth-1 and a depth-5 variation look identical.

import { indexOf } from './moveTreeIndex'
import type { Eval, MoveTree, Shape } from '../types'

/** One move, with everything the view needs to render it. */
export interface MoveCell {
  id: number
  san: string
  /** "12." / "12…" move-number prefix, or null when it continues a line. */
  number: string | null
  nags: number[]
  eval: Eval | null
  /** Rendered inline after the move — not hidden behind a marker. */
  comment: string | null
  shapes: Shape[]
  /** Alternatives at this node's parent (1 = no branch), for a "⌄N" badge. */
  siblings: number
}

/** A run of moves at one nesting depth, rendered as its own line. */
export interface MoveRow {
  /** Stable across unrelated edits: the id of the row's first move. */
  key: number
  /** 0 = mainline; +1 per variation entered. */
  depth: number
  /** The branch point this row sprouts from; null for the mainline row. */
  parentId: number | null
  cells: MoveCell[]
}

const CACHE = new WeakMap<MoveTree, MoveRow[]>()

/**
 * Rows for `tree`, memoized per tree object (same rationale as `indexOf`:
 * every mutator returns a new tree, so a cached result cannot go stale).
 */
export function treeRows(tree: MoveTree | null): MoveRow[] {
  if (!tree) return []
  const hit = CACHE.get(tree)
  if (hit) return hit
  const built = build(tree)
  CACHE.set(tree, built)
  return built
}

function build(tree: MoveTree): MoveRow[] {
  const { byId } = indexOf(tree)
  const rows: MoveRow[] = []

  /**
   * Walk the line starting at `startId`, emitting one row, and queue each
   * variation as its own row one level deeper. Iterative: a 600-ply game or a
   * deeply nested repertoire must not depend on the JS stack.
   */
  const queue: Array<{ startId: number; ply: number; depth: number; parentId: number | null }> = []

  const mainStart = byId.get(tree.root)?.children?.[0]
  if (mainStart == null) return rows
  queue.push({ startId: mainStart, ply: 1, depth: 0, parentId: tree.root })

  const seen = new Set<number>()
  while (queue.length) {
    const { startId, ply, depth, parentId } = queue.shift()!
    const cells: MoveCell[] = []
    // Variations found along this row, queued after it so a row's own moves stay
    // contiguous and its sub-lines follow in reading order.
    const branches: typeof queue = []

    let cur: number | undefined = startId
    let curPly = ply
    while (cur != null && !seen.has(cur)) {
      seen.add(cur)
      const node = byId.get(cur)
      if (!node || node.san == null) break

      const parent = node.parent != null ? byId.get(node.parent) : null
      const siblings = parent?.children.length ?? 1
      cells.push({
        id: node.id,
        san: node.san,
        number: moveNumber(curPly, cells.length === 0, cells[cells.length - 1]?.comment != null),
        nags: node.nags ?? [],
        eval: node.eval ?? null,
        comment: node.comment ?? null,
        shapes: node.shapes ?? [],
        siblings,
      })

      const [next, ...vars] = node.children
      for (const v of vars) {
        branches.push({ startId: v, ply: curPly + 1, depth: depth + 1, parentId: node.id })
      }
      cur = next
      curPly++
    }

    if (cells.length) rows.push({ key: cells[0].id, depth, parentId, cells })
    // Depth-first reading order: a variation's own sub-lines come before the
    // next sibling variation of this row.
    queue.unshift(...branches)
  }

  return rows
}

/**
 * The move-number prefix. White always carries one; Black only when it does not
 * directly follow its White move on the same line — i.e. it opens a row, or it
 * trails a comment that broke the pair up.
 */
function moveNumber(ply: number, startsRow: boolean, afterComment: boolean): string | null {
  const white = ply % 2 === 1
  const moveNo = Math.ceil(ply / 2)
  if (white) return `${moveNo}.`
  return startsRow || afterComment ? `${moveNo}…` : null
}
