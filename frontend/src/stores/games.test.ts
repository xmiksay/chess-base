import { describe, it, expect, beforeEach, vi } from 'vitest'
import { setActivePinia, createPinia } from 'pinia'

// Mock the API client so the store is tested against fakes, no network.
vi.mock('../api', () => ({
  api: {
    games: {
      list: vi.fn(),
      get: vi.fn(),
    },
  },
}))

import { api } from '../api'
import { useGamesStore } from './games'

const SCHOLARS_MATE =
  '[White "Spassky"]\n[Black "Fischer"]\n[Result "1-0"]\n\n1. e4 e5 2. Bc4 Nc6 3. Qh5 Nf6 4. Qxf7# 1-0\n'

describe('games store — list pagination', () => {
  beforeEach(() => {
    setActivePinia(createPinia())
    vi.clearAllMocks()
  })

  it('selectDatabase loads the first page and tracks the cursor', async () => {
    vi.mocked(api.games.list).mockResolvedValue({
      games: [
        { id: 1, white: 'A', black: null, result: null, date: null, eco: null, white_elo: null, black_elo: null },
        { id: 2, white: 'B', black: null, result: null, date: null, eco: null, white_elo: null, black_elo: null },
      ],
      next_cursor: 2,
    })
    const store = useGamesStore()
    await store.selectDatabase(7)

    expect(api.games.list).toHaveBeenCalledWith(7, { after: undefined })
    expect(store.games).toHaveLength(2)
    expect(store.hasMore).toBe(true)
    expect(store.loading).toBe(false)
  })

  it('loadMore appends the next page and passes the cursor', async () => {
    vi.mocked(api.games.list)
      .mockResolvedValueOnce({
        games: [
          { id: 1, white: null, black: null, result: null, date: null, eco: null, white_elo: null, black_elo: null },
          { id: 2, white: null, black: null, result: null, date: null, eco: null, white_elo: null, black_elo: null },
        ],
        next_cursor: 2,
      })
      .mockResolvedValueOnce({
        games: [
          { id: 3, white: null, black: null, result: null, date: null, eco: null, white_elo: null, black_elo: null },
        ],
        next_cursor: null,
      })
    const store = useGamesStore()
    await store.selectDatabase(7)
    await store.loadMore()

    expect(api.games.list).toHaveBeenLastCalledWith(7, { after: 2 })
    expect(store.games.map((g) => g.id)).toEqual([1, 2, 3])
    expect(store.hasMore).toBe(false)
  })

  it('selectDatabase resets a previously loaded list', async () => {
    vi.mocked(api.games.list)
      .mockResolvedValueOnce({
        games: [
          { id: 1, white: null, black: null, result: null, date: null, eco: null, white_elo: null, black_elo: null },
        ],
        next_cursor: null,
      })
      .mockResolvedValueOnce({
        games: [
          { id: 9, white: null, black: null, result: null, date: null, eco: null, white_elo: null, black_elo: null },
        ],
        next_cursor: null,
      })
    const store = useGamesStore()
    await store.selectDatabase(1)
    await store.selectDatabase(2)

    expect(store.databaseId).toBe(2)
    expect(store.games.map((g) => g.id)).toEqual([9])
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
