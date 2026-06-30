import { describe, it, expect } from 'vitest'
import { graftReviewVariations } from './reviewTree'
import { appendChild, emptyTree, getNode } from './moveTree'
import type { GameReview, MoveReview, MoveTree } from '../types'

/** Build a linear (mainline-only) tree from a list of SAN moves. */
function lineTree(sans: string[]): MoveTree {
  let tree = emptyTree()
  let parent = tree.root
  for (const san of sans) {
    const r = appendChild(tree, parent, san)
    if (!r) throw new Error('append failed')
    tree = r.tree
    parent = r.id
  }
  return tree
}

/** A review wrapping the given moves (summary is irrelevant to grafting). */
function review(moves: MoveReview[]): GameReview {
  return {
    start_fen: 'startpos',
    moves,
    summary: {
      white: { acpl: 0, accuracy: 100, inaccuracies: 0, mistakes: 0, blunders: 0 },
      black: { acpl: 0, accuracy: 100, inaccuracies: 0, mistakes: 0, blunders: 0 },
    },
  }
}

describe('graftReviewVariations', () => {
  // Mainline 1. e4 e5 2. Bc4 Nc6 → nodes 0(root) 1.e4 2.e5 3.Bc4 4.Nc6.
  const base = () => lineTree(['e4', 'e5', 'Bc4', 'Nc6'])

  it('grafts a blunder best_line as a sibling off the prior node', () => {
    const grafted = graftReviewVariations(
      base(),
      review([
        {
          ply: 4,
          san: 'Nc6',
          eval_cp: -150,
          best_move: 'Bc5',
          classification: 'mistake',
          explanation: 'Bc5 was better.',
          best_line: ['Bc5', 'd3'],
        },
      ]),
    )

    // The Bc4 node (ply 3, id 3) now has the played move plus the engine line.
    const prior = getNode(grafted, 3)!
    expect(prior.children).toHaveLength(2)
    expect(prior.children[0]).toBe(4) // mainline unchanged: Nc6 stays first

    const variationId = prior.children[1]
    const variation = getNode(grafted, variationId)!
    expect(variation.san).toBe('Bc5')
    // The line continues (Bc5 → d3).
    const cont = getNode(grafted, variation.children[0])!
    expect(cont.san).toBe('d3')
  })

  it('sets the first grafted node comment and eval', () => {
    const grafted = graftReviewVariations(
      base(),
      review([
        {
          ply: 4,
          san: 'Nc6',
          eval_cp: -150,
          best_move: 'Bc5',
          classification: 'blunder',
          explanation: 'Bc5 was better.',
          best_line: ['Bc5'],
        },
      ]),
    )
    const variation = getNode(grafted, getNode(grafted, 3)!.children[1])!
    expect(variation.comment).toBe('Bc5 was better.')
    expect(variation.eval).toEqual({ cp: -150 })
  })

  it('records a mate eval as a mate node eval', () => {
    const grafted = graftReviewVariations(
      base(),
      review([
        {
          ply: 4,
          san: 'Nc6',
          eval_cp: 1000,
          mate: 3,
          classification: 'blunder',
          explanation: 'Mate was available.',
          best_line: ['Qh5'],
        },
      ]),
    )
    const variation = getNode(grafted, getNode(grafted, 3)!.children[1])!
    expect(variation.eval).toEqual({ mate: 3 })
  })

  it('is idempotent on re-graft (no duplicate variation)', () => {
    const r = review([
      {
        ply: 4,
        san: 'Nc6',
        eval_cp: -150,
        classification: 'mistake',
        explanation: 'Bc5 was better.',
        best_line: ['Bc5', 'd3'],
      },
    ])
    const once = graftReviewVariations(base(), r)
    const twice = graftReviewVariations(once, r)
    expect(twice).toEqual(once)
    expect(getNode(twice, 3)!.children).toHaveLength(2)
  })

  it('leaves non-critical moves untouched', () => {
    const grafted = graftReviewVariations(
      base(),
      review([
        {
          ply: 2,
          san: 'e5',
          eval_cp: 20,
          classification: 'good',
          explanation: '',
          best_line: ['c5', 'Nf3'],
        },
        { ply: 1, san: 'e4', eval_cp: 20, classification: 'best', explanation: '', best_line: ['e4'] },
      ]),
    )
    // No grafts: the tree is structurally unchanged (still linear, 5 nodes).
    expect(grafted.nodes).toHaveLength(5)
    expect(getNode(grafted, 1)!.children).toEqual([2])
    expect(getNode(grafted, 2)!.children).toEqual([3])
  })

  it('skips critical moves without a best_line', () => {
    const grafted = graftReviewVariations(
      base(),
      review([{ ply: 4, san: 'Nc6', eval_cp: -150, classification: 'mistake', explanation: 'bad' }]),
    )
    expect(grafted.nodes).toHaveLength(5)
    expect(getNode(grafted, 3)!.children).toEqual([4])
  })
})
