import { describe, it, expect } from 'vitest'
import type { DrawShape } from 'chessground/draw'
import { composeBoardShapes, shapesToDrawShapes, overlayBrushes } from './boardShapes'

const plan: DrawShape = { orig: 'g1', dest: 'f3', brush: 'plan1' }
const threat: DrawShape = { orig: 'd6', dest: 'e5', brush: 'threat' }
const master: DrawShape = { orig: 'e2', dest: 'e4', brush: 'master' }

describe('composeBoardShapes', () => {
  it('includes only enabled layers, in plans→threats→master order', () => {
    const out = composeBoardShapes(
      { plans: [plan], threats: [threat], master: [master] },
      { plans: true, threats: true, master: true },
    )
    expect(out).toEqual([plan, threat, master])
  })

  it('excludes a disabled layer from the union', () => {
    const out = composeBoardShapes(
      { plans: [plan], threats: [threat], master: [master] },
      { plans: true, threats: false, master: false },
    )
    expect(out).toEqual([plan])
  })

  it('is empty when every layer is off', () => {
    const out = composeBoardShapes(
      { plans: [plan], threats: [threat], master: [master] },
      { plans: false, threats: false, master: false },
    )
    expect(out).toEqual([])
  })

  it('tolerates missing layer arrays', () => {
    expect(composeBoardShapes({}, { plans: true, threats: true, master: true })).toEqual([])
  })
})

describe('shapesToDrawShapes', () => {
  it('maps arrows (orig+dest) and highlights (orig only)', () => {
    const out = shapesToDrawShapes([
      { orig: 'd6', dest: 'e5', brush: 'threat' },
      { orig: 'e4', dest: null, brush: 'threat' },
    ])
    expect(out).toEqual([
      { orig: 'd6', dest: 'e5', brush: 'threat' },
      { orig: 'e4', brush: 'threat' },
    ])
  })

  it('returns [] for a non-array input', () => {
    expect(shapesToDrawShapes(undefined as never)).toEqual([])
  })
})

describe('overlayBrushes', () => {
  it('defines the threat (red) and master (violet) brushes', () => {
    const b = overlayBrushes()
    expect(b.threat.color).toMatch(/^#/)
    expect(b.master.color).toMatch(/^#/)
  })
})
