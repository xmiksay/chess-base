import { describe, it, expect } from 'vitest'
import { treeRows } from './moveTreeRows'
import type { MoveNode, MoveTree } from '../types'

function node(
  id: number,
  parent: number | null,
  san: string | null,
  children: number[],
  extra: Partial<MoveNode> = {},
): MoveNode {
  return { id, parent, san, comment: null, nags: [], children, ...extra }
}

/** 1.e4 e5 2.Nf3  with 1...c5 as a variation carrying 2.Nc3 d6 */
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

describe('treeRows', () => {
  it('returns nothing for an empty or absent tree', () => {
    expect(treeRows(null)).toEqual([])
    expect(treeRows({ root: 0, nodes: [node(0, null, null, [])] })).toEqual([])
  })

  it('puts the whole mainline on one row', () => {
    const [main] = treeRows(branching())
    expect(main.depth).toBe(0)
    expect(main.parentId).toBe(0)
    expect(main.cells.map((c) => c.san)).toEqual(['e4', 'e5', 'Nf3'])
  })

  it('gives each variation its own row one level deeper', () => {
    const rows = treeRows(branching())
    expect(rows).toHaveLength(2)
    expect(rows[1].depth).toBe(1)
    expect(rows[1].parentId).toBe(1) // sprouts from 1.e4
    expect(rows[1].cells.map((c) => c.san)).toEqual(['c5', 'Nc3', 'd6'])
  })

  // The defect this model exists to fix: the old renderer styled depth 0 one way
  // and *everything else* another, so depth 1 and depth 5 were indistinguishable.
  it('reports increasing depth for nested variations', () => {
    const nested: MoveTree = {
      root: 0,
      nodes: [
        node(0, null, null, [1]),
        node(1, 0, 'e4', [2, 3]),
        node(2, 1, 'e5', []),
        node(3, 1, 'c5', [4, 5]),
        node(4, 3, 'Nf3', []),
        node(5, 3, 'Nc3', [6, 7]),
        node(6, 5, 'd6', []),
        node(7, 5, 'Nc6', [8, 9]),
        node(8, 7, 'Bb5', []),
        node(9, 7, 'g3', []),
      ],
    }
    // Each branch point opens a row one level deeper than the row it left.
    expect(treeRows(nested).map((r) => r.depth)).toEqual([0, 1, 2, 3, 4])
  })

  it('numbers White always and Black only when it opens a row or trails a comment', () => {
    const rows = treeRows(branching())
    expect(rows[0].cells.map((c) => c.number)).toEqual(['1.', null, '2.'])
    // The variation row opens on a Black move, so it needs the ellipsis form.
    expect(rows[1].cells[0].number).toBe('1…')
  })

  it('numbers a Black move that trails a comment', () => {
    const commented: MoveTree = {
      root: 0,
      nodes: [
        node(0, null, null, [1]),
        node(1, 0, 'e4', [2], { comment: 'Best by test' }),
        node(2, 1, 'e5', []),
      ],
    }
    const [row] = treeRows(commented)
    expect(row.cells[1].number).toBe('1…')
  })

  it('carries the annotations the view renders inline', () => {
    const annotated: MoveTree = {
      root: 0,
      nodes: [
        node(0, null, null, [1]),
        node(1, 0, 'e4', [], {
          comment: 'Sicilian',
          nags: [5],
          eval: { cp: 27 },
          shapes: [{ orig: 'e2', dest: 'e4', brush: 'green' }],
        }),
      ],
    }
    const cell = treeRows(annotated)[0].cells[0]
    expect(cell.comment).toBe('Sicilian')
    expect(cell.nags).toEqual([5])
    expect(cell.eval).toEqual({ cp: 27 })
    expect(cell.shapes).toHaveLength(1)
  })

  it('counts siblings so a branch point is visible without clicking', () => {
    const rows = treeRows(branching())
    expect(rows[0].cells[0].siblings).toBe(1) // 1.e4 is the only first move
    expect(rows[0].cells[1].siblings).toBe(2) // 1...e5 has 1...c5 alongside
  })

  it('keys rows stably on their first move', () => {
    const rows = treeRows(branching())
    expect(rows.map((r) => r.key)).toEqual([1, 3])
  })

  it('memoizes per tree object', () => {
    const tree = branching()
    expect(treeRows(tree)).toBe(treeRows(tree))
  })

  it('terminates on a malformed cyclic tree', () => {
    const cyclic: MoveTree = {
      root: 0,
      nodes: [node(0, null, null, [1]), node(1, 0, 'e4', [1])],
    }
    expect(treeRows(cyclic)[0].cells.map((c) => c.san)).toEqual(['e4'])
  })

  it('handles a long line without recursing', () => {
    const nodes: MoveNode[] = [node(0, null, null, [1])]
    for (let i = 1; i <= 600; i++) {
      nodes.push(node(i, i - 1, i % 2 ? 'Nf3' : 'Nf6', i < 600 ? [i + 1] : []))
    }
    expect(treeRows({ root: 0, nodes })[0].cells).toHaveLength(600)
  })
})
