import { describe, it, expect } from 'vitest'
import { sideToMove, placement, STARTPOS_FEN } from './fen.js'

describe('fen helpers', () => {
  it('reads side to move', () => {
    expect(sideToMove(STARTPOS_FEN)).toBe('white')
    expect(
      sideToMove('rnbqkbnr/pppppppp/8/8/4P3/8/PPPP1PPP/RNBQKBNR b KQkq - 0 1'),
    ).toBe('black')
  })

  it('extracts the placement field', () => {
    expect(placement(STARTPOS_FEN)).toBe(
      'rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR',
    )
  })
})
