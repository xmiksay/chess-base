import { describe, it, expect } from 'vitest'
import { masterMovesToShapes, MAX_MASTER_ARROWS } from './masterShapes'
import { STARTPOS_FEN } from './fen'
import type { MoveStat } from '../types'

function stat(san: string, count: number): MoveStat {
  return { san, count, white: count, draws: 0, black: 0 }
}

describe('masterMovesToShapes', () => {
  it('maps the top continuations to arrows sized + labelled by frequency', () => {
    const shapes = masterMovesToShapes(STARTPOS_FEN, [
      stat('e4', 60),
      stat('d4', 30),
      stat('Nf3', 10),
    ])
    expect(shapes).toHaveLength(3)
    // Sorted by count desc; e4 is the most-played.
    expect(shapes[0]).toMatchObject({ orig: 'e2', dest: 'e4', brush: 'master' })
    expect(shapes[0].label?.text).toBe('60%')
    // 60% is thick, 30% thick, 10% medium → widths decrease with frequency.
    expect(shapes[0].modifiers?.lineWidth).toBeGreaterThanOrEqual(shapes[2].modifiers!.lineWidth!)
    expect(shapes[2].label?.text).toBe('10%')
    expect(shapes[2]).toMatchObject({ orig: 'g1', dest: 'f3' })
  })

  it('caps the number of arrows', () => {
    const many = Array.from({ length: 10 }, (_, i) => stat(`a${(i % 8) + 1}`, 10 - i))
    expect(masterMovesToShapes(STARTPOS_FEN, many).length).toBeLessThanOrEqual(MAX_MASTER_ARROWS)
  })

  it('skips rows whose SAN is illegal in the position', () => {
    const shapes = masterMovesToShapes(STARTPOS_FEN, [stat('e4', 50), stat('Qh5', 50)])
    // Qh5 is illegal from the start position and is dropped.
    expect(shapes).toHaveLength(1)
    expect(shapes[0].dest).toBe('e4')
  })

  it('returns [] for empty stats or an unparseable FEN', () => {
    expect(masterMovesToShapes(STARTPOS_FEN, [])).toEqual([])
    expect(masterMovesToShapes('not a fen', [stat('e4', 1)])).toEqual([])
  })
})
