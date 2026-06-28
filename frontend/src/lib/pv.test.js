import { describe, it, expect } from 'vitest'
import { parseUci, uciLineToSan } from './pv.js'
import { STARTPOS_FEN } from './fen.js'

describe('parseUci', () => {
  it('splits a plain move', () => {
    expect(parseUci('e2e4')).toEqual({ from: 'e2', to: 'e4' })
  })

  it('reads the promotion piece', () => {
    expect(parseUci('e7e8q')).toEqual({ from: 'e7', to: 'e8', promotion: 'q' })
  })

  it('rejects junk', () => {
    expect(parseUci('e2')).toBeNull()
    expect(parseUci(null)).toBeNull()
  })
})

describe('uciLineToSan', () => {
  it('renders a legal line as SAN from the start position', () => {
    expect(uciLineToSan(STARTPOS_FEN, ['e2e4', 'e7e5', 'g1f3'])).toEqual(['e4', 'e5', 'Nf3'])
  })

  it('stops at the first illegal move', () => {
    expect(uciLineToSan(STARTPOS_FEN, ['e2e4', 'e2e4'])).toEqual(['e4'])
  })

  it('caps the number of plies', () => {
    expect(uciLineToSan(STARTPOS_FEN, ['e2e4', 'e7e5', 'g1f3'], 2)).toEqual(['e4', 'e5'])
  })

  it('returns [] for an invalid FEN or non-array pv', () => {
    expect(uciLineToSan('garbage', ['e2e4'])).toEqual([])
    expect(uciLineToSan(STARTPOS_FEN, null)).toEqual([])
  })
})
