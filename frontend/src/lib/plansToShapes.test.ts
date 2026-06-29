import { describe, it, expect } from 'vitest'
import { plansToShapes, planBrushes, MAX_PLAN_LINES } from './plansToShapes'
import type { PlanLine } from '../types'

function line(multipv: number, trajectories: PlanLine['trajectories']): PlanLine {
  return { multipv, depth: 20, score: { type: 'cp', value: 30 }, pv: [], trajectories }
}

describe('plansToShapes', () => {
  it('emits one arrow per consecutive square pair of a trajectory', () => {
    const shapes = plansToShapes([line(1, [{ piece: 'N', squares: ['g1', 'f3', 'g5'] }])])
    expect(shapes).toEqual([
      { orig: 'g1', dest: 'f3', brush: 'plan1' },
      { orig: 'f3', dest: 'g5', brush: 'plan1' },
    ])
  })

  it('brushes each line by ascending-MultiPV rank', () => {
    const shapes = plansToShapes([
      line(2, [{ piece: 'b', squares: ['c8', 'g4'] }]),
      line(1, [{ piece: 'N', squares: ['g1', 'f3'] }]),
    ])
    expect(shapes.map((s) => s.brush)).toEqual(['plan1', 'plan2'])
    expect(shapes[0]).toMatchObject({ orig: 'g1', dest: 'f3' })
  })

  it('dims every non-active line when active is set', () => {
    const lines = [
      line(1, [{ piece: 'N', squares: ['g1', 'f3'] }]),
      line(2, [{ piece: 'B', squares: ['f1', 'c4'] }]),
    ]
    const shapes = plansToShapes(lines, { active: 2 })
    expect(shapes.find((s) => s.orig === 'g1')?.brush).toBe('plan1d')
    expect(shapes.find((s) => s.orig === 'f1')?.brush).toBe('plan2')
  })

  it('labels only the first segment with the 1-based piece order', () => {
    const shapes = plansToShapes(
      [
        line(1, [
          { piece: 'N', squares: ['g1', 'f3', 'g5'] },
          { piece: 'B', squares: ['f1', 'c4'] },
        ]),
      ],
      { labels: true },
    )
    expect(shapes[0].label).toEqual({ text: '1' })
    expect(shapes[1].label).toBeUndefined() // second segment of the knight path
    expect(shapes[2].label).toEqual({ text: '2' }) // bishop's first segment
  })

  it('caps the overlay at MAX_PLAN_LINES lines', () => {
    const lines = Array.from({ length: 5 }, (_, i) =>
      line(i + 1, [{ piece: 'N', squares: ['g1', 'f3'] }]),
    )
    const shapes = plansToShapes(lines)
    expect(shapes).toHaveLength(MAX_PLAN_LINES)
  })

  it('skips degenerate and malformed trajectories', () => {
    expect(plansToShapes([line(1, [{ piece: 'N', squares: ['e4'] }])])).toEqual([])
    expect(plansToShapes([line(1, [{ piece: 'N', squares: ['e4', 'e4'] }])])).toEqual([])
    expect(
      plansToShapes([line(1, [{ piece: 'N', squares: null as unknown as string[] }])]),
    ).toEqual([])
  })

  it('returns [] for non-array input', () => {
    expect(plansToShapes(null as unknown as PlanLine[])).toEqual([])
  })
})

describe('planBrushes', () => {
  it('defines a full and dimmed brush per line, dim having lower opacity', () => {
    const brushes = planBrushes()
    for (let n = 1; n <= MAX_PLAN_LINES; n++) {
      expect(brushes[`plan${n}`].color).toBe(brushes[`plan${n}d`].color)
      expect(brushes[`plan${n}d`].opacity).toBeLessThan(brushes[`plan${n}`].opacity)
    }
  })
})
