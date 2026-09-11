import { describe, it, expect } from 'vitest'
import { isPromotion, PROMOTION_PIECES } from './promotion'
import { STARTPOS_FEN } from './fen'

const WHITE_PUSH = '4k3/3P4/8/8/8/8/8/4K3 w - - 0 1'
const BLACK_PUSH = '4k3/8/8/8/8/8/4p3/4K3 b - - 0 1'

describe('isPromotion', () => {
  it('detects a white pawn reaching the 8th rank', () => {
    expect(isPromotion(WHITE_PUSH, 'd7', 'd8')).toBe(true)
  })

  it('detects a black pawn reaching the 1st rank', () => {
    expect(isPromotion(BLACK_PUSH, 'e2', 'e1')).toBe(true)
  })

  it('detects a promoting capture onto the last rank', () => {
    expect(isPromotion(WHITE_PUSH, 'd7', 'e8')).toBe(true)
  })

  it('is false for a pawn move short of the last rank', () => {
    expect(isPromotion(STARTPOS_FEN, 'e2', 'e4')).toBe(false)
  })

  it('is false for a non-pawn reaching the last rank', () => {
    const rook = '4k3/R7/8/8/8/8/8/4K3 w - - 0 1'
    expect(isPromotion(rook, 'a7', 'a8')).toBe(false)
  })

  it('is false when the origin square is empty', () => {
    expect(isPromotion(WHITE_PUSH, 'a7', 'a8')).toBe(false)
  })

  it("is false for the opponent's pawn on its own last rank", () => {
    // Black to move, so a white pawn on d7 is not the mover.
    const blackToMove = '4k3/3P4/8/8/8/8/8/4K3 b - - 0 1'
    expect(isPromotion(blackToMove, 'd7', 'd8')).toBe(false)
  })

  it('offers queen first, then the underpromotions', () => {
    expect([...PROMOTION_PIECES]).toEqual(['q', 'r', 'b', 'n'])
  })
})
