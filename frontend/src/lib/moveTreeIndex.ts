// One-pass lookup tables over a backend `MoveTree`, memoized on the tree object.
//
// `moveTree.ts`'s navigation helpers were all written against `nodes.find(…)`,
// so `getNode` was O(n) and anything calling it per ply — `mainlinePath`,
// `sanPath`, `childWithSan` — was O(n²). On a merged repertoire or a danger map
// that is thousands of nodes walked on every cursor step.
//
// Memoizing is safe because every mutator in `moveTree.ts` (`appendChild`,
// `reorderChild`, `deleteSubtree`, …) returns a *new* `MoveTree` object rather
// than mutating in place, so a cached index can never outlive the tree it
// describes. A WeakMap also means a dropped tree takes its index with it.

import type { MoveNode, MoveTree } from '../types'

export interface TreeIndex {
  /** Every node by id — replaces the linear `nodes.find`. */
  byId: Map<number, MoveNode>
  /** Mainline node ids, root first; the array index is the ply. */
  mainline: number[]
  /** Ply of each mainline node (the inverse of `mainline`). */
  plyOf: Map<number, number>
  /** Variation nesting depth: 0 on the mainline, +1 per branch taken. */
  depthOf: Map<number, number>
}

const CACHE = new WeakMap<MoveTree, TreeIndex>()

/**
 * The index for `tree`, built once per tree object. Callers may hold the result
 * only as long as they hold that exact tree — after any edit the tree identity
 * changes and this returns a fresh index.
 */
export function indexOf(tree: MoveTree): TreeIndex {
  const hit = CACHE.get(tree)
  if (hit) return hit
  const built = build(tree)
  CACHE.set(tree, built)
  return built
}

function build(tree: MoveTree): TreeIndex {
  const byId = new Map<number, MoveNode>()
  for (const n of tree.nodes) byId.set(n.id, n)

  // `plyOf` doubles as the visited set: a malformed tree whose children[0] chain
  // loops must terminate here rather than growing the array until it throws.
  const mainline: number[] = []
  const plyOf = new Map<number, number>()
  for (let cur: number | undefined = tree.root; cur != null && !plyOf.has(cur); ) {
    plyOf.set(cur, mainline.length)
    mainline.push(cur)
    cur = byId.get(cur)?.children?.[0]
  }

  // Iterative DFS: a deeply nested tree must not blow the stack, and the depth
  // of a node is its parent's depth plus one for every non-mainline branch.
  const depthOf = new Map<number, number>()
  const stack: Array<{ id: number; depth: number }> = [{ id: tree.root, depth: 0 }]
  const seen = new Set<number>()
  while (stack.length) {
    const { id, depth } = stack.pop()!
    if (seen.has(id)) continue // a malformed tree must not spin forever
    seen.add(id)
    depthOf.set(id, depth)
    const children = byId.get(id)?.children ?? []
    for (let i = 0; i < children.length; i++) {
      stack.push({ id: children[i], depth: i === 0 ? depth : depth + 1 })
    }
  }

  return { byId, mainline, plyOf, depthOf }
}
