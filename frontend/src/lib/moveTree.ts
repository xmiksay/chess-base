// Pure helpers over a backend `MoveTree` (`{ root, nodes }`, see src/pgn_tree.rs).
// Each node is `{ id, parent, san, comment, nags, children }`; `children[0]` is
// the mainline continuation, the rest are variations. Framework-free so the
// tree-navigation + move-append logic is unit-testable without a board.

import type { MoveNode, MoveToken, MoveTree } from '../types'

/** A fresh tree holding only the root sentinel (san=null = the start position). */
export function emptyTree(): MoveTree {
  return {
    root: 0,
    nodes: [{ id: 0, parent: null, san: null, comment: null, nags: [], children: [] }],
  }
}

/** One past the highest node id — ids stay unique across appends and deletes. */
function nextId(tree: MoveTree): number {
  return tree.nodes.reduce((max, n) => Math.max(max, n.id), 0) + 1
}

/**
 * Append `san` as a new child of `parentId`, returning the new tree and node id.
 * `children[0]` is the mainline, so the first child of a node is its mainline
 * continuation and any later child is a variation — branching, not truncation.
 * Returns null when `parentId` is absent.
 */
export function appendChild(
  tree: MoveTree,
  parentId: number,
  san: string,
): { tree: MoveTree; id: number } | null {
  if (!getNode(tree, parentId)) return null
  const id = nextId(tree)
  const nodes = tree.nodes.map((n) =>
    n.id === parentId ? { ...n, children: [...n.children, id] } : n,
  )
  nodes.push({ id, parent: parentId, san, comment: null, nags: [], children: [] })
  return { tree: { root: tree.root, nodes }, id }
}

/**
 * Remove `id` and its whole subtree, returning the new tree and the parent the
 * caller should select next. The root sentinel cannot be deleted (no-op).
 */
export function deleteSubtree(tree: MoveTree, id: number): { tree: MoveTree; parentId: number } {
  if (id === tree.root) return { tree, parentId: tree.root }
  const map = nodeMap(tree)
  const node = map.get(id)
  if (!node) return { tree, parentId: tree.root }
  const parentId = node.parent ?? tree.root
  const doomed = new Set<number>()
  const stack = [id]
  while (stack.length) {
    const cur = stack.pop() as number
    doomed.add(cur)
    const n = map.get(cur)
    if (n) stack.push(...n.children)
  }
  const nodes = tree.nodes
    .filter((n) => !doomed.has(n.id))
    .map((n) =>
      n.id === parentId ? { ...n, children: n.children.filter((c) => c !== id) } : n,
    )
  return { tree: { root: tree.root, nodes }, parentId }
}

/** Index a tree's nodes by id (ids are dense but we never assume id === index). */
export function nodeMap(tree: MoveTree): Map<number, MoveNode> {
  const m = new Map<number, MoveNode>()
  for (const n of tree.nodes) m.set(n.id, n)
  return m
}

/** The node with `id`, or null when it is absent. */
export function getNode(tree: MoveTree, id: number): MoveNode | null {
  return tree.nodes.find((n) => n.id === id) ?? null
}

/** Mainline child of `id` (`children[0]`), or null at a leaf / missing node. */
export function firstChild(tree: MoveTree, id: number): number | null {
  const node = getNode(tree, id)
  return node?.children?.[0] ?? null
}

/** A child of `id` whose move equals `san`, or null — used to dedupe replays. */
export function childWithSan(tree: MoveTree, id: number, san: string): number | null {
  const node = getNode(tree, id)
  if (!node) return null
  const map = nodeMap(tree)
  for (const c of node.children) {
    if (map.get(c)?.san === san) return c
  }
  return null
}

/** SAN moves from the root down to `id` (the line that reaches that node). */
export function sanPath(tree: MoveTree, id: number): string[] {
  const map = nodeMap(tree)
  const sans: string[] = []
  let cur = map.get(id)
  while (cur && cur.parent != null) {
    if (cur.san) sans.push(cur.san)
    cur = map.get(cur.parent)
  }
  return sans.reverse()
}

/**
 * Node ids along the mainline: the root, then `firstChild` repeatedly down to
 * the leaf. The array index is the ply (index 0 = root = the start position),
 * which is why grafting variations — appended as later children, never as
 * `children[0]` — leaves this mapping stable.
 */
export function mainlinePath(tree: MoveTree): number[] {
  const path: number[] = []
  let cur: number | null = tree.root
  while (cur != null) {
    path.push(cur)
    cur = firstChild(tree, cur)
  }
  return path
}

/** Follow the mainline from `id` to its leaf, returning the leaf node id. */
export function lastMainlineId(tree: MoveTree, id: number): number {
  let cur = id
  let next = firstChild(tree, cur)
  while (next != null) {
    cur = next
    next = firstChild(tree, cur)
  }
  return cur
}

/** Standard glyph for a NAG code, falling back to `$n` for unmapped ones. */
const NAG_GLYPHS: Record<number, string> = { 1: '!', 2: '?', 3: '!!', 4: '??', 5: '!?', 6: '?!' }
export function nagGlyph(n: number): string {
  return NAG_GLYPHS[n] ?? `$${n}`
}

/**
 * Flatten the tree into a linear token stream for rendering: `move` tokens
 * interleaved with `open` / `close` tokens that bracket variations. Move tokens
 * carry everything the view needs (`san`, `nags`, `comment`, the move-number
 * prefix, and nesting `depth`) so the component stays presentational.
 */
export function treeTokens(tree: MoveTree | null): MoveToken[] {
  const tokens: MoveToken[] = []
  if (!tree) return tokens
  const map = nodeMap(tree)

  function pushMove(id: number, ply: number, depth: number): void {
    const node = map.get(id)
    if (!node) return
    const white = ply % 2 === 1
    const moveNo = Math.ceil(ply / 2)
    const prev = tokens[tokens.length - 1]
    let number: string | null = null
    if (white) {
      number = `${moveNo}.`
    } else {
      // Black moves only need a number when they don't directly follow a White
      // move on the same line — i.e. they start a (variation), resume after one
      // closes, or trail a comment.
      const detached =
        !prev || prev.type === 'open' || prev.type === 'close' || (prev.type === 'move' && prev.comment)
      if (detached) number = `${moveNo}…`
    }
    tokens.push({
      type: 'move',
      id,
      san: node.san,
      nags: node.nags ?? [],
      comment: node.comment ?? null,
      number,
      depth,
    })
  }

  function walkChildren(parentId: number, parentPly: number, depth: number): void {
    const parent = map.get(parentId)
    if (!parent || !parent.children.length) return
    const [main, ...vars] = parent.children
    const ply = parentPly + 1
    pushMove(main, ply, depth)
    for (const v of vars) {
      tokens.push({ type: 'open', depth: depth + 1 })
      pushMove(v, ply, depth + 1)
      walkChildren(v, ply, depth + 1)
      tokens.push({ type: 'close', depth: depth + 1 })
    }
    walkChildren(main, ply, depth)
  }

  walkChildren(tree.root, 0, 0)
  return tokens
}
