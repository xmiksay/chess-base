import { describe, it, expect } from 'vitest'
import type { MoveToken } from '../types'
import {
  appendChild,
  childWithSan,
  deleteSubtree,
  emptyTree,
  firstChild,
  getNode,
  lastMainlineId,
  nagGlyph,
  sanPath,
  treeTokens,
} from './moveTree'

// 1.e4 e5 (1...c5!? {Sicilian} 2.Nf3) 2.Nf3 — a mainline with one variation.
function sampleTree() {
  return {
    root: 0,
    nodes: [
      { id: 0, parent: null, san: null, comment: null, nags: [], children: [1] },
      { id: 1, parent: 0, san: 'e4', comment: null, nags: [], children: [2, 4] },
      { id: 2, parent: 1, san: 'e5', comment: null, nags: [], children: [3] },
      { id: 3, parent: 2, san: 'Nf3', comment: null, nags: [], children: [] },
      { id: 4, parent: 1, san: 'c5', comment: 'Sicilian', nags: [5], children: [5] },
      { id: 5, parent: 4, san: 'Nf3', comment: null, nags: [], children: [] },
    ],
  }
}

describe('moveTree navigation', () => {
  const tree = sampleTree()

  it('getNode / firstChild walk the mainline', () => {
    expect(getNode(tree, 2)!.san).toBe('e5')
    expect(getNode(tree, 99)).toBeNull()
    expect(firstChild(tree, 1)).toBe(2)
    expect(firstChild(tree, 3)).toBeNull()
  })

  it('childWithSan finds the variation branch, else null', () => {
    expect(childWithSan(tree, 1, 'c5')).toBe(4)
    expect(childWithSan(tree, 1, 'e5')).toBe(2)
    expect(childWithSan(tree, 1, 'Nf3')).toBeNull()
  })

  it('sanPath returns the line reaching a node', () => {
    expect(sanPath(tree, 3)).toEqual(['e4', 'e5', 'Nf3'])
    expect(sanPath(tree, 5)).toEqual(['e4', 'c5', 'Nf3'])
    expect(sanPath(tree, 0)).toEqual([])
  })

  it('lastMainlineId follows children[0] to the leaf', () => {
    expect(lastMainlineId(tree, 0)).toBe(3)
    expect(lastMainlineId(tree, 4)).toBe(5)
  })

  it('nagGlyph maps known codes and falls back to $n', () => {
    expect(nagGlyph(1)).toBe('!')
    expect(nagGlyph(6)).toBe('?!')
    expect(nagGlyph(42)).toBe('$42')
  })
})

describe('tree mutators', () => {
  it('emptyTree holds just the root sentinel', () => {
    const t = emptyTree()
    expect(t.root).toBe(0)
    expect(t.nodes).toHaveLength(1)
    expect(t.nodes[0]).toMatchObject({ id: 0, parent: null, san: null, children: [] })
  })

  it('appendChild adds a mainline child, then a variation sibling', () => {
    let t = emptyTree()
    const a = appendChild(t, 0, 'e4')!
    t = a.tree
    expect(getNode(t, a.id)).toMatchObject({ san: 'e4', parent: 0 })
    expect(getNode(t, 0)!.children).toEqual([a.id])

    const main = appendChild(t, a.id, 'e5')!
    t = main.tree
    const variation = appendChild(t, a.id, 'c5')!
    t = variation.tree
    // children[0] stays the mainline; the later append is a variation.
    expect(getNode(t, a.id)!.children).toEqual([main.id, variation.id])
    expect(firstChild(t, a.id)).toBe(main.id)
  })

  it('appendChild returns null for a missing parent and never mutates input', () => {
    const t = emptyTree()
    expect(appendChild(t, 42, 'e4')).toBeNull()
    expect(t.nodes).toHaveLength(1) // input untouched
  })

  it('appendChild never collides with a live node id', () => {
    let t = emptyTree()
    t = appendChild(t, 0, 'e4')!.tree // id 1
    const second = appendChild(t, 1, 'e5')! // id 2
    t = deleteSubtree(second.tree, second.id).tree // remove id 2 (now free)
    const added = appendChild(t, 1, 'c5')!
    t = added.tree
    // The freed id may be reclaimed, but it must be unique among the live nodes.
    const ids = t.nodes.map((n) => n.id)
    expect(new Set(ids).size).toBe(ids.length)
    expect(getNode(t, added.id)!.san).toBe('c5')
  })

  it('deleteSubtree removes a node with its descendants and returns the parent', () => {
    const { tree, parentId } = deleteSubtree(sampleTree(), 4) // the c5 variation
    expect(parentId).toBe(1)
    expect(getNode(tree, 4)).toBeNull()
    expect(getNode(tree, 5)).toBeNull() // descendant gone too
    expect(getNode(tree, 1)!.children).toEqual([2]) // unlinked from parent
    expect(getNode(tree, 2)!.san).toBe('e5') // siblings survive
  })

  it('deleteSubtree refuses to remove the root and no-ops on a missing id', () => {
    const a = deleteSubtree(sampleTree(), 0)
    expect(a.parentId).toBe(0)
    expect(a.tree.nodes).toHaveLength(6)
    const b = deleteSubtree(sampleTree(), 99)
    expect(b.tree.nodes).toHaveLength(6)
  })
})

describe('treeTokens', () => {
  it('flattens mainline + bracketed variation with move numbers', () => {
    const tokens = treeTokens(sampleTree())
    const shape = tokens.map((t) =>
      t.type === 'move' ? { san: t.san, number: t.number, depth: t.depth } : t.type,
    )
    expect(shape).toEqual([
      { san: 'e4', number: '1.', depth: 0 },
      { san: 'e5', number: null, depth: 0 },
      'open',
      { san: 'c5', number: '1…', depth: 1 },
      { san: 'Nf3', number: '2.', depth: 1 },
      'close',
      { san: 'Nf3', number: '2.', depth: 0 },
    ])
  })

  it('carries comments and nags on move tokens', () => {
    const c5 = treeTokens(sampleTree()).find(
      (t): t is Extract<MoveToken, { type: 'move' }> => t.type === 'move' && t.san === 'c5',
    )
    expect(c5!.comment).toBe('Sicilian')
    expect(c5!.nags).toEqual([5])
  })

  it('returns an empty stream for a null or move-less tree', () => {
    expect(treeTokens(null)).toEqual([])
    expect(
      treeTokens({
        root: 0,
        nodes: [{ id: 0, parent: null, san: null, comment: null, nags: [], children: [] }],
      }),
    ).toEqual(
      [],
    )
  })
})
