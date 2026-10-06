import { describe, it, expect } from 'vitest'
import { indexOf } from './moveTreeIndex'
import { emptyTree, appendChild, mainlinePath, getNode } from './moveTree'
import type { MoveNode, MoveTree } from '../types'

function node(id: number, parent: number | null, san: string | null, children: number[]): MoveNode {
  return { id, parent, san, comment: null, nags: [], children }
}

/** root -e4- 1 ┬ e5 (2) ── Nf3 (4)
 *              └ c5 (3) ── Nc3 (5) ── d6 (6)   */
function branching(): MoveTree {
  return {
    root: 0,
    nodes: [
      node(0, null, null, [1]),
      node(1, 0, 'e4', [2, 3]),
      node(2, 1, 'e5', [4]),
      node(3, 1, 'c5', [5]),
      node(4, 2, 'Nf3', []),
      node(5, 3, 'Nc3', [6]),
      node(6, 5, 'd6', []),
    ],
  }
}

function line(sans: string[]): MoveTree {
  let tree = emptyTree()
  let parent = tree.root
  for (const san of sans) {
    const r = appendChild(tree, parent, san)!
    tree = r.tree
    parent = r.id
  }
  return tree
}

describe('moveTreeIndex', () => {
  it('indexes every node by id', () => {
    const idx = indexOf(branching())
    expect(idx.byId.size).toBe(7)
    expect(idx.byId.get(5)!.san).toBe('Nc3')
    expect(idx.byId.get(99)).toBeUndefined()
  })

  it('walks the mainline via children[0] and maps each node to its ply', () => {
    const idx = indexOf(branching())
    expect(idx.mainline).toEqual([0, 1, 2, 4])
    expect(idx.plyOf.get(0)).toBe(0)
    expect(idx.plyOf.get(4)).toBe(3)
    // A variation node is not on the mainline and has no ply.
    expect(idx.plyOf.get(3)).toBeUndefined()
  })

  it('counts variation depth, not tree depth', () => {
    const idx = indexOf(branching())
    expect(idx.depthOf.get(1)).toBe(0) // mainline
    expect(idx.depthOf.get(4)).toBe(0) // still mainline, 3 plies down
    expect(idx.depthOf.get(3)).toBe(1) // first variation
    expect(idx.depthOf.get(6)).toBe(1) // deeper, but the same variation
  })

  it('reuses the index for the same tree object and rebuilds for a new one', () => {
    const tree = branching()
    expect(indexOf(tree)).toBe(indexOf(tree))
    // Every moveTree mutator returns a NEW tree, which is what makes the cache
    // safe — a fresh object must get a fresh index.
    const { tree: grown } = appendChild(tree, 4, 'Nc6')!
    expect(indexOf(grown)).not.toBe(indexOf(tree))
    expect(indexOf(grown).byId.size).toBe(8)
    expect(indexOf(tree).byId.size).toBe(7)
  })

  it('survives a cycle in the child graph instead of spinning', () => {
    const cyclic: MoveTree = {
      root: 0,
      nodes: [node(0, null, null, [1]), node(1, 0, 'e4', [0])],
    }
    // Both walks are cycle-guarded; a loop terminates instead of growing the
    // mainline array until it throws (which is what the original did).
    const idx = indexOf(cyclic)
    expect(idx.mainline).toEqual([0, 1])
    expect(idx.depthOf.size).toBe(2)
  })

  it('handles a long line in one pass', () => {
    const sans = Array.from({ length: 400 }, (_, i) => (i % 2 ? 'Nf6' : 'Nf3'))
    const tree = line(['e4', 'e5', ...sans])
    const idx = indexOf(tree)
    expect(idx.mainline).toHaveLength(403) // root + 402 plies
    expect(idx.plyOf.get(idx.mainline[402])).toBe(402)
  })

  it('backs the public moveTree helpers unchanged', () => {
    const tree = branching()
    expect(mainlinePath(tree)).toEqual(indexOf(tree).mainline)
    expect(getNode(tree, 5)).toBe(indexOf(tree).byId.get(5))
    expect(getNode(tree, 99)).toBeNull()
  })
})
