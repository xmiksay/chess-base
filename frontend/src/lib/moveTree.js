// Pure helpers over a backend `MoveTree` (`{ root, nodes }`, see src/pgn_tree.rs).
// Each node is `{ id, parent, san, comment, nags, children }`; `children[0]` is
// the mainline continuation, the rest are variations. Framework-free so the
// tree-navigation + move-append logic is unit-testable without a board.

/** Index a tree's nodes by id (ids are dense but we never assume id === index). */
export function nodeMap(tree) {
  const m = new Map()
  for (const n of tree.nodes) m.set(n.id, n)
  return m
}

/** The node with `id`, or null when it is absent. */
export function getNode(tree, id) {
  return tree.nodes.find((n) => n.id === id) ?? null
}

/** Mainline child of `id` (`children[0]`), or null at a leaf / missing node. */
export function firstChild(tree, id) {
  const node = getNode(tree, id)
  return node?.children?.[0] ?? null
}

/** A child of `id` whose move equals `san`, or null — used to dedupe replays. */
export function childWithSan(tree, id, san) {
  const node = getNode(tree, id)
  if (!node) return null
  const map = nodeMap(tree)
  for (const c of node.children) {
    if (map.get(c)?.san === san) return c
  }
  return null
}

/** SAN moves from the root down to `id` (the line that reaches that node). */
export function sanPath(tree, id) {
  const map = nodeMap(tree)
  const sans = []
  let cur = map.get(id)
  while (cur && cur.parent != null) {
    if (cur.san) sans.push(cur.san)
    cur = map.get(cur.parent)
  }
  return sans.reverse()
}

/** Follow the mainline from `id` to its leaf, returning the leaf node id. */
export function lastMainlineId(tree, id) {
  let cur = id
  let next = firstChild(tree, cur)
  while (next != null) {
    cur = next
    next = firstChild(tree, cur)
  }
  return cur
}

/** Standard glyph for a NAG code, falling back to `$n` for unmapped ones. */
const NAG_GLYPHS = { 1: '!', 2: '?', 3: '!!', 4: '??', 5: '!?', 6: '?!' }
export function nagGlyph(n) {
  return NAG_GLYPHS[n] ?? `$${n}`
}

/**
 * Flatten the tree into a linear token stream for rendering: `move` tokens
 * interleaved with `open` / `close` tokens that bracket variations. Move tokens
 * carry everything the view needs (`san`, `nags`, `comment`, the move-number
 * prefix, and nesting `depth`) so the component stays presentational.
 */
export function treeTokens(tree) {
  const tokens = []
  if (!tree) return tokens
  const map = nodeMap(tree)

  function pushMove(id, ply, depth) {
    const node = map.get(id)
    if (!node) return
    const white = ply % 2 === 1
    const moveNo = Math.ceil(ply / 2)
    const prev = tokens[tokens.length - 1]
    let number = null
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

  function walkChildren(parentId, parentPly, depth) {
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
