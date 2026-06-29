import { describe, it, expect } from 'vitest'
import { positionsFromPgn, clampPly, navigate } from './pgnViewer'
import { STARTPOS_FEN } from './fen'

const SCHOLARS_MATE =
  '[White "Spassky"]\n[Black "Fischer"]\n[Result "1-0"]\n\n1. e4 e5 2. Bc4 Nc6 3. Qh5 Nf6 4. Qxf7# 1-0\n'

describe('positionsFromPgn', () => {
  it('replays a PGN into one position per ply, index 0 being the start', () => {
    const positions = positionsFromPgn(SCHOLARS_MATE)
    // 7 plies + the start position.
    expect(positions).toHaveLength(8)
    expect(positions[0]).toMatchObject({ ply: 0, san: null, fen: STARTPOS_FEN, lastMove: null })
    expect(positions[1].san).toBe('e4')
    expect(positions[1].lastMove).toEqual(['e2', 'e4'])
    expect(positions[1].fen).toContain(' b ') // black to move after 1. e4
    expect(positions.at(-1)!.san).toBe('Qxf7#')
  })

  it('returns a single start position for an empty or unparseable PGN', () => {
    for (const pgn of ['', '   ', null, undefined, 'not a game']) {
      const positions = positionsFromPgn(pgn)
      expect(positions).toHaveLength(1)
      expect(positions[0].ply).toBe(0)
      expect(positions[0].lastMove).toBeNull()
    }
  })
})

describe('clampPly', () => {
  it('clamps into [0, total-1]', () => {
    expect(clampPly(-5, 8)).toBe(0)
    expect(clampPly(100, 8)).toBe(7)
    expect(clampPly(3, 8)).toBe(3)
  })

  it('handles empty lists and non-finite input', () => {
    expect(clampPly(2, 0)).toBe(0)
    expect(clampPly(NaN, 8)).toBe(0)
    expect(clampPly(2.9, 8)).toBe(2) // truncates toward zero
  })
})

describe('navigate', () => {
  const total = 8 // plies 0..7

  it('steps prev/next without leaving range', () => {
    expect(navigate(0, 'prev', total)).toBe(0)
    expect(navigate(0, 'next', total)).toBe(1)
    expect(navigate(7, 'next', total)).toBe(7)
    expect(navigate(5, 'prev', total)).toBe(4)
  })

  it('jumps to first and last', () => {
    expect(navigate(5, 'first', total)).toBe(0)
    expect(navigate(2, 'last', total)).toBe(7)
  })

  it('goes to an explicit ply, clamped', () => {
    expect(navigate(0, 3, total)).toBe(3)
    expect(navigate(0, 99, total)).toBe(7)
    expect(navigate(0, -2, total)).toBe(0)
  })
})
