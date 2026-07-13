import { describe, it, expect } from 'vitest'
import type { MoveStat } from '../types'
import {
  START_FEN,
  formatMoveStat,
  frequency,
  lineFen,
  moveToSan,
  replayLine,
  scoreBar,
  totalCount,
} from './openingTree'

describe('replayLine', () => {
  it('returns the start position for an empty line', () => {
    const state = replayLine([])
    expect(state.fen).toBe(START_FEN)
    expect(state.lastMove).toBeNull()
    expect(state.turnColor).toBe('white')
    expect(state.plies).toBe(0)
    expect(state.ok).toBe(true)
  })

  it('replays a legal line and tracks the last move + side to move', () => {
    const state = replayLine(['e4', 'e5', 'Nf3'])
    expect(state.fen).toContain(' b ') // black to move after 3 plies
    expect(state.turnColor).toBe('black')
    expect(state.lastMove).toEqual(['g1', 'f3'])
    expect(state.plies).toBe(3)
    expect(state.ok).toBe(true)
  })

  it('exposes legal destinations as a from→[to] map', () => {
    const { dests } = replayLine([])
    expect(dests.get('e2')).toContain('e4')
    expect(dests.get('g1')).toContain('f3')
  })

  it('stops at the first illegal move without throwing', () => {
    const state = replayLine(['e4', 'Qh5', 'e5']) // Qh5 illegal for black on move 1.5
    expect(state.ok).toBe(false)
    // Only the legal prefix (e4) applied.
    expect(state.plies).toBe(1)
    expect(state.lastMove).toEqual(['e2', 'e4'])
  })
})

describe('lineFen', () => {
  it('matches the start FEN at the root', () => {
    expect(lineFen([])).toBe(START_FEN)
  })

  it('advances as moves are appended', () => {
    expect(lineFen(['e4'])).not.toBe(START_FEN)
    expect(lineFen(['e4'])).toContain(' b ') // black to move after 1.e4
  })
})

describe('moveToSan', () => {
  it('translates a legal board drag into SAN', () => {
    expect(moveToSan([], 'e2', 'e4')).toBe('e4')
    expect(moveToSan(['e4', 'e5'], 'g1', 'f3')).toBe('Nf3')
  })

  it('returns null for an illegal drag', () => {
    expect(moveToSan([], 'e2', 'e5')).toBeNull()
  })

  it('honors the promotion piece', () => {
    // White pawn on e7 promoting on e8 (set up via a quick line is awkward, so
    // assert the default queen path through a normal capture-free promotion FEN).
    const san = moveToSan(
      ['e4', 'd5', 'exd5', 'c6', 'dxc6', 'Nf6', 'cxb7', 'Nbd7'],
      'b7',
      'a8',
      'q',
    )
    expect(san).toMatch(/^bxa8=Q/)
  })
})

describe('scoreBar', () => {
  it('splits decided games into integer percentages summing to 100', () => {
    const bar = scoreBar({ white: 3, draws: 1, black: 0 } as MoveStat)
    expect(bar.white + bar.draws + bar.black).toBe(100)
    expect(bar.white).toBe(75)
    expect(bar.draws).toBe(25)
    expect(bar.black).toBe(0)
  })

  it('ignores unknown results in the denominator', () => {
    // count carries unknowns, but only 2 games are decided (1 white, 1 black)
    // → 50 / 0 / 50 over those decided games.
    const bar = scoreBar({ count: 4, white: 1, draws: 0, black: 1 } as MoveStat)
    expect(bar).toEqual({ white: 50, draws: 0, black: 50 })
  })

  it('reads 0/0/0 when no game is decided', () => {
    expect(scoreBar({ white: 0, draws: 0, black: 0 } as MoveStat)).toEqual({ white: 0, draws: 0, black: 0 })
  })
})

describe('totalCount / frequency', () => {
  const tree = [
    { san: 'e4', count: 6 },
    { san: 'd4', count: 3 },
    { san: 'Nf3', count: 1 },
  ] as MoveStat[]

  it('totals counts across the tree', () => {
    expect(totalCount(tree)).toBe(10)
  })

  it('computes a move’s share as an integer percentage', () => {
    expect(frequency(tree[0], 10)).toBe(60)
    expect(frequency(tree[1], 10)).toBe(30)
  })

  it('is zero when the total is zero', () => {
    expect(frequency({ count: 5 } as MoveStat, 0)).toBe(0)
  })
})

describe('formatMoveStat', () => {
  it('formats a compact "N games, WW/DD/LL" string', () => {
    const stat = { san: 'e4', count: 12, white: 8, draws: 2, black: 2 } as MoveStat
    expect(formatMoveStat(stat)).toBe('12 games, 8W/2D/2L')
  })

  it('uses the singular "game" for a count of one', () => {
    const stat = { san: 'e4', count: 1, white: 1, draws: 0, black: 0 } as MoveStat
    expect(formatMoveStat(stat)).toBe('1 game, 1W/0D/0L')
  })
})
