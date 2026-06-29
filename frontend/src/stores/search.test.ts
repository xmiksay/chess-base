import { describe, it, expect, vi, beforeEach } from 'vitest'
import { setActivePinia, createPinia } from 'pinia'
import { useSearchStore } from './search'
import { api } from '../api'
import { START_FEN } from '../lib/openingTree'

vi.mock('../api', () => ({
  api: {
    search: {
      headers: vi.fn(),
      tree: vi.fn(),
      games: vi.fn(),
    },
  },
}))

beforeEach(() => {
  setActivePinia(createPinia())
  vi.clearAllMocks()
  vi.mocked(api.search.headers).mockResolvedValue({ games: [], next_cursor: null })
  vi.mocked(api.search.tree).mockResolvedValue([])
  vi.mocked(api.search.games).mockResolvedValue([])
})

describe('header search', () => {
  it('runs a search with the mapped params and stores results', async () => {
    vi.mocked(api.search.headers).mockResolvedValue({
      games: [
        {
          id: 1,
          white: 'Carlsen',
          black: null,
          result: null,
          date: null,
          eco: null,
          white_elo: null,
          black_elo: null,
        },
      ],
      next_cursor: null,
    })
    const store = useSearchStore()
    store.query.player = '  Carlsen  '
    store.query.dateFrom = '2020.01.01'

    await store.runHeaderSearch()

    expect(api.search.headers).toHaveBeenCalledWith({
      player: 'Carlsen',
      date_from: '2020.01.01',
    })
    expect(store.results).toHaveLength(1)
    expect(store.searched).toBe(true)
    expect(store.hasMore).toBe(false)
    expect(store.headerError).toBeNull()
  })

  it('paginates: loadMore appends the next page and tracks the cursor', async () => {
    vi.mocked(api.search.headers).mockResolvedValueOnce({
      games: [
        {
          id: 1,
          white: null,
          black: null,
          result: null,
          date: null,
          eco: null,
          white_elo: null,
          black_elo: null,
        },
      ],
      next_cursor: 'c1',
    })
    const store = useSearchStore()
    await store.runHeaderSearch()
    expect(store.hasMore).toBe(true)
    expect(store.nextCursor).toBe('c1')

    vi.mocked(api.search.headers).mockResolvedValueOnce({
      games: [
        {
          id: 2,
          white: null,
          black: null,
          result: null,
          date: null,
          eco: null,
          white_elo: null,
          black_elo: null,
        },
      ],
      next_cursor: null,
    })
    await store.loadMore()

    // The second call echoes the previous page's cursor.
    expect(api.search.headers).toHaveBeenLastCalledWith({ cursor: 'c1' })
    expect(store.results.map((g) => g.id)).toEqual([1, 2])
    expect(store.hasMore).toBe(false)
  })

  it('captures errors and clears results', async () => {
    vi.mocked(api.search.headers).mockRejectedValue(new Error('boom'))
    const store = useSearchStore()
    await store.runHeaderSearch()
    expect(store.headerError).toBe('boom')
    expect(store.results).toEqual([])
  })

  it('resetQuery clears state', async () => {
    const store = useSearchStore()
    store.query.event = 'Candidates'
    store.results = [
      {
        id: 1,
        white: null,
        black: null,
        result: null,
        date: null,
        eco: null,
        white_elo: null,
        black_elo: null,
      },
    ]
    store.searched = true
    store.resetQuery()
    expect(store.queryIsEmpty).toBe(true)
    expect(store.results).toEqual([])
    expect(store.searched).toBe(false)
  })
})

describe('opening-tree navigation', () => {
  it('descends a move, querying the new position', async () => {
    vi.mocked(api.search.tree).mockResolvedValue([
      { san: 'e5', count: 2, white: 1, draws: 0, black: 1 },
    ])
    const store = useSearchStore()

    await store.playSan('e4')

    expect(store.line).toEqual(['e4'])
    expect(store.fen).toContain(' b ') // black to move after 1.e4
    // The position queried was the post-e4 FEN, not the start.
    const queried = vi.mocked(api.search.tree).mock.calls.at(-1)![0]
    expect(queried).not.toBe(START_FEN)
    expect(store.tree).toHaveLength(1)
  })

  it('back() pops one move and reloads', async () => {
    const store = useSearchStore()
    await store.playSan('e4')
    await store.playSan('e5')
    expect(store.line).toEqual(['e4', 'e5'])

    await store.back()
    expect(store.line).toEqual(['e4'])
  })

  it('back() at the root is a no-op', async () => {
    const store = useSearchStore()
    await store.back()
    expect(store.line).toEqual([])
  })

  it('resetBoard() returns to the start position', async () => {
    const store = useSearchStore()
    await store.playSan('e4')
    await store.resetBoard()
    expect(store.line).toEqual([])
    expect(store.fen).toBe(START_FEN)
  })

  it('playMove() translates a legal drag to SAN and descends', async () => {
    const store = useSearchStore()
    const san = store.playMove({ from: 'e2', to: 'e4' })
    expect(san).toBe('e4')
    expect(store.line).toEqual(['e4'])
  })

  it('playMove() ignores an illegal drag', () => {
    const store = useSearchStore()
    const san = store.playMove({ from: 'e2', to: 'e5' })
    expect(san).toBeNull()
    expect(store.line).toEqual([])
  })

  it('surfaces explorer errors', async () => {
    vi.mocked(api.search.tree).mockRejectedValue(new Error('offline'))
    const store = useSearchStore()
    await store.loadPosition()
    expect(store.explorerError).toBe('offline')
    expect(store.tree).toEqual([])
  })
})
