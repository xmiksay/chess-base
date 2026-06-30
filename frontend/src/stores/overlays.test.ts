import { describe, it, expect, vi, beforeEach } from 'vitest'
import { setActivePinia, createPinia } from 'pinia'
import { useOverlaysStore } from './overlays'
import { api } from '../api'
import { STARTPOS_FEN } from '../lib/fen'

vi.mock('../api', () => ({
  api: {
    search: { threats: vi.fn(), tree: vi.fn() },
  },
}))

beforeEach(() => {
  vi.clearAllMocks()
  setActivePinia(createPinia())
})

describe('overlays store', () => {
  it('loadThreats maps the backend Shape[] to chessground draw shapes', async () => {
    vi.mocked(api.search.threats).mockResolvedValue([{ orig: 'd6', dest: 'e5', brush: 'threat' }])
    const s = useOverlaysStore()
    await s.loadThreats('4k3/8/3p4/4N3/8/8/8/4K3 w - - 0 1')
    expect(s.threats).toEqual([{ orig: 'd6', dest: 'e5', brush: 'threat' }])
  })

  it('loadThreats clears and records the error on failure', async () => {
    vi.mocked(api.search.threats).mockRejectedValue(new Error('boom'))
    const s = useOverlaysStore()
    s.threats = [{ orig: 'a1', dest: 'a2', brush: 'threat' }]
    await s.loadThreats('x')
    expect(s.threats).toEqual([])
    expect(s.error).toContain('boom')
  })

  it('loadMaster turns /api/search/tree stats into master arrows', async () => {
    vi.mocked(api.search.tree).mockResolvedValue([
      { san: 'e4', count: 80, white: 40, draws: 20, black: 20 },
      { san: 'd4', count: 20, white: 10, draws: 5, black: 5 },
    ])
    const s = useOverlaysStore()
    await s.loadMaster(STARTPOS_FEN)
    expect(s.master).toHaveLength(2)
    expect(s.master[0]).toMatchObject({ orig: 'e2', dest: 'e4', brush: 'master' })
    expect(s.master[0].label?.text).toBe('80%')
  })

  it('clearThreats / clearMaster empty their layers', () => {
    const s = useOverlaysStore()
    s.threats = [{ orig: 'a1', dest: 'a2', brush: 'threat' }]
    s.master = [{ orig: 'e2', dest: 'e4', brush: 'master' }]
    s.clearThreats()
    s.clearMaster()
    expect(s.threats).toEqual([])
    expect(s.master).toEqual([])
  })
})
