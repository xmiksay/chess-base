import { describe, it, expect, beforeEach, vi } from 'vitest'
import { setActivePinia, createPinia } from 'pinia'

// Mock the API client so the store is tested against fakes, no network.
vi.mock('../api', () => ({
  api: {
    games: {
      list: vi.fn(),
      get: vi.fn(),
      exportPgn: vi.fn(),
    },
  },
}))

import { api } from '../api'
import { useGamesStore } from './games'

const SCHOLARS_MATE =
  '[White "Spassky"]\n[Black "Fischer"]\n[Result "1-0"]\n\n1. e4 e5 2. Bc4 Nc6 3. Qh5 Nf6 4. Qxf7# 1-0\n'

const row = (id: number) => ({
  id,
  white: null,
  black: null,
  result: null,
  date: null,
  eco: null,
  white_elo: null,
  black_elo: null,
})

describe('games store — list pagination', () => {
  beforeEach(() => {
    setActivePinia(createPinia())
    vi.clearAllMocks()
  })

  it('selectDatabase loads the first page with the default sort', async () => {
    vi.mocked(api.games.list).mockResolvedValue({
      games: [row(2), row(1)],
      total: 2,
      page: 0,
      limit: 50,
    })
    const store = useGamesStore()
    await store.selectDatabase(7)

    expect(api.games.list).toHaveBeenCalledWith(7, {
      page: 0,
      limit: 50,
      sort: 'date',
      dir: 'desc',
    })
    expect(store.games).toHaveLength(2)
    expect(store.total).toBe(2)
    expect(store.pageCount).toBe(1)
    expect(store.hasNext).toBe(false)
    expect(store.loading).toBe(false)
  })

  it('goToPage requests the next offset page and tracks position', async () => {
    vi.mocked(api.games.list)
      .mockResolvedValueOnce({ games: [row(5), row(4)], total: 3, page: 0, limit: 2 })
      .mockResolvedValueOnce({ games: [row(3)], total: 3, page: 1, limit: 2 })
    const store = useGamesStore()
    await store.selectDatabase(7)
    expect(store.hasNext).toBe(true)

    await store.goToPage(1)
    expect(api.games.list).toHaveBeenLastCalledWith(7, {
      page: 1,
      limit: 2,
      sort: 'date',
      dir: 'desc',
    })
    expect(store.games.map((g) => g.id)).toEqual([3])
    expect(store.page).toBe(1)
    expect(store.hasNext).toBe(false)
    expect(store.hasPrev).toBe(true)
  })

  it('setSort flips direction on the active field and resets to page 0', async () => {
    vi.mocked(api.games.list).mockResolvedValue({ games: [], total: 0, page: 0, limit: 50 })
    const store = useGamesStore()
    await store.selectDatabase(7)

    // Active field 'date' starts desc → click flips to asc.
    await store.setSort('date')
    expect(store.dir).toBe('asc')
    expect(api.games.list).toHaveBeenLastCalledWith(7, {
      page: 0,
      limit: 50,
      sort: 'date',
      dir: 'asc',
    })

    // A different field starts at its natural default (eco → asc).
    await store.setSort('eco')
    expect(store.sort).toBe('eco')
    expect(store.dir).toBe('asc')
  })

  it('records an error when a page fails to load', async () => {
    vi.mocked(api.games.list).mockRejectedValue(new Error('boom'))
    const store = useGamesStore()
    await store.selectDatabase(7)

    expect(store.error).toBe('boom')
    expect(store.loading).toBe(false)
  })
})

describe('games store — move navigation', () => {
  beforeEach(() => {
    setActivePinia(createPinia())
    vi.clearAllMocks()
  })

  it('open loads a game and starts at the initial position', async () => {
    vi.mocked(api.games.get).mockResolvedValue({
      id: 5,
      white: 'Spassky',
      black: null,
      result: null,
      date: null,
      eco: null,
      white_elo: null,
      black_elo: null,
      pgn: SCHOLARS_MATE,
    })
    const store = useGamesStore()
    await store.open(5)

    expect(store.openGame!.id).toBe(5)
    expect(store.positions).toHaveLength(8) // 7 plies + start
    expect(store.ply).toBe(0)
    expect(store.atStart).toBe(true)
    expect(store.atEnd).toBe(false)
  })

  it('go steps through plies and clamps at the ends', async () => {
    vi.mocked(api.games.get).mockResolvedValue({
      id: 5,
      white: null,
      black: null,
      result: null,
      date: null,
      eco: null,
      white_elo: null,
      black_elo: null,
      pgn: SCHOLARS_MATE,
    })
    const store = useGamesStore()
    await store.open(5)

    store.go('next')
    expect(store.ply).toBe(1)
    expect(store.lastMove).toEqual(['e2', 'e4'])

    store.go('prev')
    store.go('prev') // already at start → clamped
    expect(store.ply).toBe(0)

    store.go('last')
    expect(store.ply).toBe(7)
    expect(store.atEnd).toBe(true)
    store.go('next') // clamped at end
    expect(store.ply).toBe(7)

    store.go(3)
    expect(store.ply).toBe(3)
  })
})

describe('games store — PGN export', () => {
  beforeEach(() => {
    setActivePinia(createPinia())
    vi.clearAllMocks()
  })

  async function openGame(store: ReturnType<typeof useGamesStore>) {
    vi.mocked(api.games.get).mockResolvedValue({
      id: 5,
      white: null,
      black: null,
      result: null,
      date: null,
      eco: null,
      white_elo: null,
      black_elo: null,
      pgn: SCHOLARS_MATE,
    })
    await store.open(5)
  }

  it('exportPgn returns the open game PGN and passes the annotated flag', async () => {
    vi.mocked(api.games.exportPgn).mockResolvedValue('1. e4 e5 *')
    const store = useGamesStore()
    await openGame(store)

    await expect(store.exportPgn(true)).resolves.toBe('1. e4 e5 *')
    expect(api.games.exportPgn).toHaveBeenCalledWith(5, { annotated: true })
  })

  it('exportPgn returns null and records the error on failure', async () => {
    vi.mocked(api.games.exportPgn).mockRejectedValue(new Error('no engine'))
    const store = useGamesStore()
    await openGame(store)

    await expect(store.exportPgn(true)).resolves.toBeNull()
    expect(store.error).toBe('no engine')
  })

  it('exportPgn is a no-op with no open game', async () => {
    const store = useGamesStore()
    await expect(store.exportPgn()).resolves.toBeNull()
    expect(api.games.exportPgn).not.toHaveBeenCalled()
  })
})
