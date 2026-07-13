import { describe, it, expect } from 'vitest'
import {
  brushForRole,
  dangerBrushes,
  dangerRoles,
  dangerShapesForFen,
  formatEval,
} from './dangerShapes'
import { STARTPOS_FEN } from './fen'
import type { DangerNode, DangerTree } from '../types'

// A two-ply spine: root → 1.e4 → 1...e5, with the dangerous reply 2.Qh5 flagged at
// the position after 1...e5 (a Caution trap). FENs are the real positions so SAN
// resolution against the board works exactly like masterShapes.
const FEN_AFTER_E4 = 'rnbqkbnr/pppppppp/8/8/4P3/8/PPPP1PPP/RNBQKBNR b KQkq - 0 1'
const FEN_AFTER_E5 = 'rnbqkbnr/pppp1ppp/8/4p3/4P3/8/PPPP1PPP/RNBQKBNR w KQkq - 0 2'

function node(over: Partial<DangerNode> & Pick<DangerNode, 'id' | 'fen'>): DangerNode {
  return { parent: null, ply: 0, children: [], ...over }
}

function sampleTree(): DangerTree {
  return {
    root: 0,
    nodes: [
      node({ id: 0, fen: STARTPOS_FEN }),
      node({ id: 1, parent: 0, san: 'e4', fen: FEN_AFTER_E4, ply: 1 }),
      node({ id: 2, parent: 1, san: 'e5', fen: FEN_AFTER_E5, ply: 2 }),
      node({
        id: 3,
        parent: 2,
        san: 'Qh5',
        fen: 'rnbqkbnr/pppp1ppp/8/4p2Q/4P3/8/PPPP1PPP/RNB1KBNR b KQkq - 1 2',
        ply: 3,
        tag: {
          kind: 'Trap',
          role: 'Caution',
          only_move_gap: 150,
          miss_rate: 0.4,
          eval: { cp: -40 },
        },
      }),
    ],
  }
}

describe('dangerShapes', () => {
  it('maps a danger role to its brush key, defaulting to off-book', () => {
    expect(brushForRole('Weapon')).toBe('dangerWeapon')
    expect(brushForRole('Caution')).toBe('dangerCaution')
    expect(brushForRole('OffBook')).toBe('dangerOffbook')
    expect(brushForRole('whatever')).toBe('dangerOffbook')
  })

  it('defines a brush for every role', () => {
    const b = dangerBrushes()
    expect(b.dangerWeapon.color).toBe('#15803d')
    expect(b.dangerCaution.color).toBe('#dc2626')
    expect(b.dangerOffbook.color).toBe('#d97706')
  })

  it('draws an arrow for a flagged reply whose parent sits at the given FEN', () => {
    const shapes = dangerShapesForFen(sampleTree(), FEN_AFTER_E5)
    expect(shapes).toEqual([{ orig: 'd1', dest: 'h5', brush: 'dangerCaution' }])
  })

  it('draws nothing at a position with no flagged children', () => {
    expect(dangerShapesForFen(sampleTree(), FEN_AFTER_E4)).toEqual([])
  })

  it('ignores clock differences when matching the position', () => {
    // Same position as FEN_AFTER_E5 but with bumped half/full-move counters.
    const stale = FEN_AFTER_E5.replace('- 0 2', '- 9 9')
    const shapes = dangerShapesForFen(sampleTree(), stale)
    expect(shapes).toEqual([{ orig: 'd1', dest: 'h5', brush: 'dangerCaution' }])
  })

  it('is empty for a null tree or an unparseable FEN', () => {
    expect(dangerShapesForFen(null, FEN_AFTER_E5)).toEqual([])
    expect(dangerShapesForFen(sampleTree(), 'not-a-fen')).toEqual([])
  })

  it('flattens only the flagged nodes into panel rows with their figures', () => {
    const rows = dangerRoles(sampleTree())
    expect(rows).toEqual([
      {
        nodeId: 3,
        san: 'Qh5',
        // The full line + a move-numbered label, so the panel can disambiguate
        // and navigate to it in the study tree.
        line: ['e4', 'e5', 'Qh5'],
        label: '2.Qh5',
        kind: 'Trap',
        role: 'Caution',
        onlyMoveGap: 150,
        missRate: 0.4,
        trap: null,
        attack: null,
        eval: { cp: -40 },
      },
    ])
  })

  it('formats an eval as a signed pawn score or a mate count', () => {
    expect(formatEval({ cp: 30 })).toBe('+0.30')
    expect(formatEval({ cp: -40 })).toBe('-0.40')
    expect(formatEval({ cp: 0 })).toBe('+0.00')
    expect(formatEval({ mate: 3 })).toBe('M3')
    expect(formatEval({ mate: -2 })).toBe('-M2')
  })
})
