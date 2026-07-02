import { describe, it, expect, vi, beforeEach } from 'vitest'
import { setActivePinia, createPinia } from 'pinia'
import { useDangerStore } from './danger'
import { api } from '../api'
import { STARTPOS_FEN } from '../lib/fen'
import type { DangerWalkResult } from '../types'

vi.mock('../api', () => ({
  api: {
    studies: { dangerMap: vi.fn() },
  },
}))

beforeEach(() => {
  vi.clearAllMocks()
  setActivePinia(createPinia())
})

const result: DangerWalkResult = {
  tree: {
    root: 0,
    nodes: [
      { id: 0, parent: null, fen: STARTPOS_FEN, ply: 0, children: [1] },
      {
        id: 1,
        parent: 0,
        san: 'Qh5',
        fen: STARTPOS_FEN,
        ply: 1,
        children: [],
        tag: { kind: 'Trap', role: 'Caution', miss_rate: 0.5 },
      },
    ],
  },
  roles: [{ node_id: 1, san: 'Qh5', kind: 'Trap', role: 'Caution' }],
}

describe('danger store', () => {
  it('load populates the tree and flattens the panel roles', async () => {
    vi.mocked(api.studies.dangerMap).mockResolvedValue(result)
    const s = useDangerStore()
    await s.load({ spine_pgn: '1. e4 c5 *' })
    expect(s.tree?.nodes).toHaveLength(2)
    expect(s.roles).toEqual([
      {
        nodeId: 1,
        san: 'Qh5',
        line: ['Qh5'],
        label: '1.Qh5',
        kind: 'Trap',
        role: 'Caution',
        onlyMoveGap: null,
        missRate: 0.5,
        trap: null,
        attack: null,
      },
    ])
    expect(s.error).toBeNull()
  })

  it('load clears the tree and records the error on failure', async () => {
    vi.mocked(api.studies.dangerMap).mockRejectedValue(new Error('no engine'))
    const s = useDangerStore()
    await s.load({ spine_pgn: 'x' })
    expect(s.tree).toBeNull()
    expect(s.roles).toEqual([])
    expect(s.error).toContain('no engine')
  })

  it('clear empties the loaded map', async () => {
    vi.mocked(api.studies.dangerMap).mockResolvedValue(result)
    const s = useDangerStore()
    await s.load({ spine_pgn: '1. e4 c5 *' })
    s.clear()
    expect(s.tree).toBeNull()
    expect(s.roles).toEqual([])
  })
})
