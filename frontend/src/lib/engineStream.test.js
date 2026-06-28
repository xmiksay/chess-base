import { describe, it, expect } from 'vitest'
import {
  parseEngineMessage,
  reduceInfo,
  sortedLines,
  scoreToWhiteCp,
  formatScore,
  evalBarPercent,
} from './engineStream.js'

describe('parseEngineMessage', () => {
  it('parses a well-formed event', () => {
    expect(parseEngineMessage('{"type":"ready","name":"SF"}')).toEqual({
      type: 'ready',
      name: 'SF',
    })
  })

  it('rejects non-JSON and objects without a string type', () => {
    expect(parseEngineMessage('not json')).toBeNull()
    expect(parseEngineMessage('{"name":"x"}')).toBeNull()
    expect(parseEngineMessage('42')).toBeNull()
  })
})

describe('reduceInfo', () => {
  it('keys lines by multipv and keeps them sorted', () => {
    const lines = new Map()
    reduceInfo(lines, { type: 'info', multipv: 2, depth: 10, score: { type: 'cp', value: -20 }, pv: ['e7e5'] })
    reduceInfo(lines, { type: 'info', multipv: 1, depth: 10, score: { type: 'cp', value: 30 }, pv: ['e2e4'] })
    const out = sortedLines(lines)
    expect(out.map((l) => l.multipv)).toEqual([1, 2])
    expect(out[0].pv).toEqual(['e2e4'])
    expect(out[0].score).toEqual({ type: 'cp', value: 30 })
  })

  it('defaults missing multipv to line 1 and maps time_ms', () => {
    const lines = new Map()
    reduceInfo(lines, { type: 'info', depth: 5, score: { type: 'cp', value: 0 }, time_ms: 200, pv: ['d2d4'] })
    expect(lines.get(1).timeMs).toBe(200)
  })

  it('ignores partial info with neither score nor pv', () => {
    const lines = new Map()
    reduceInfo(lines, { type: 'info', depth: 1 })
    expect(lines.size).toBe(0)
  })

  it('carries forward prior fields when a later line omits them', () => {
    const lines = new Map()
    reduceInfo(lines, { type: 'info', multipv: 1, depth: 8, nps: 1000, score: { type: 'cp', value: 10 }, pv: ['e2e4'] })
    reduceInfo(lines, { type: 'info', multipv: 1, depth: 12, score: { type: 'cp', value: 15 }, pv: ['e2e4', 'e7e5'] })
    const line = lines.get(1)
    expect(line.depth).toBe(12)
    expect(line.nps).toBe(1000) // preserved
  })
})

describe('score perspective helpers', () => {
  it('keeps White-to-move scores as-is and flips Black-to-move scores', () => {
    expect(scoreToWhiteCp({ type: 'cp', value: 50 }, 'white')).toBe(50)
    expect(scoreToWhiteCp({ type: 'cp', value: 50 }, 'black')).toBe(-50)
  })

  it('formats centipawns with a sign', () => {
    expect(formatScore({ type: 'cp', value: 124 }, 'white')).toBe('+1.24')
    expect(formatScore({ type: 'cp', value: 124 }, 'black')).toBe('-1.24')
    expect(formatScore({ type: 'cp', value: 0 }, 'white')).toBe('0.00')
  })

  it('formats mates from White perspective', () => {
    expect(formatScore({ type: 'mate', value: 3 }, 'white')).toBe('M3')
    expect(formatScore({ type: 'mate', value: 3 }, 'black')).toBe('-M3')
    expect(formatScore(null, 'white')).toBe('—')
  })

  it('maps eval to a 0–100 bar percentage', () => {
    expect(evalBarPercent({ type: 'cp', value: 0 }, 'white')).toBe(50)
    expect(evalBarPercent({ type: 'mate', value: 1 }, 'white')).toBe(100)
    expect(evalBarPercent({ type: 'mate', value: 1 }, 'black')).toBe(0)
    expect(evalBarPercent({ type: 'cp', value: 800 }, 'white')).toBeGreaterThan(90)
    expect(evalBarPercent(null, 'white')).toBe(50)
  })
})
