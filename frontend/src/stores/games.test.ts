import { describe, it, expect, beforeEach, vi } from 'vitest'
import { setActivePinia, createPinia } from 'pinia'

// Mock the API client so the store is tested against fakes, no network.
vi.mock('../api', () => ({
  api: {
    games: {
      list: vi.fn(),
      get: vi.fn(),
      tree: vi.fn(),
      exportPgn: vi.fn(),
      saveAsStudy: vi.fn(),
      linkedStudies: vi.fn(),
    },
  },
}))

import { api } from '../api'
import { useGamesStore } from './games'
import { appendChild, emptyTree } from '../lib/moveTree'
import type { GameReview, MoveTree } from '../types'

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

const detail = (id: number) => ({ ...row(id), pgn: '' })

/** Build a linear tree from SAN moves (root id 0, then 1, 2, … in order). */
function lineTree(sans: string[]): MoveTree {
  let tree = emptyTree()
  let parent = tree.root
  for (const san of sans) {
    const r = appendChild(tree, parent, san)!
    tree = r.tree
    parent = r.id
  }
  return tree
}

describe('games store — list pagination', () => {
  beforeEach(() => {
    setActivePinia(createPinia())
    vi.clearAllMocks()
    vi.mocked(api.games.linkedStudies).mockResolvedValue([])
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

describe('games store — tree board', () => {
  beforeEach(() => {
    setActivePinia(createPinia())
    vi.clearAllMocks()
    vi.mocked(api.games.linkedStudies).mockResolvedValue([])
  })

  async function openLine(store: ReturnType<typeof useGamesStore>, sans: string[]) {
    vi.mocked(api.games.get).mockResolvedValue(detail(5))
    vi.mocked(api.games.tree).mockResolvedValue(lineTree(sans))
    await store.open(5)
  }

  it('open seeds the board from /tree at the start position', async () => {
    const store = useGamesStore()
    await openLine(store, ['e4', 'e5', 'Bc4'])

    expect(api.games.tree).toHaveBeenCalledWith(5)
    expect(store.openGame!.id).toBe(5)
    expect(store.tree.nodes).toHaveLength(4) // root + 3 plies
    expect(store.currentId).toBe(store.tree.root)
    expect(store.atStart).toBe(true)
    expect(store.atEnd).toBe(false)
  })

  it('maps mainline nodes to plies and back', async () => {
    const store = useGamesStore()
    await openLine(store, ['e4', 'e5', 'Bc4'])

    expect(store.mainlinePath()).toEqual([0, 1, 2, 3])
    expect(store.plyOf(0)).toBe(0)
    expect(store.plyOf(3)).toBe(3)
    expect(store.nodeAtPly(2)).toBe(2)
    expect(store.nodeAtPly(99)).toBeNull()
  })

  it('navigates by node id and the cursor controls', async () => {
    const store = useGamesStore()
    await openLine(store, ['e4', 'e5', 'Bc4'])

    store.next()
    expect(store.currentId).toBe(1)
    expect(store.lastMove).toEqual(['e2', 'e4'])

    store.goto(3)
    expect(store.currentId).toBe(3)
    expect(store.atEnd).toBe(true)

    store.prev()
    expect(store.currentId).toBe(2)
    store.first()
    expect(store.currentId).toBe(0)
    store.last()
    expect(store.currentId).toBe(3)
  })

  it('playing an off-line move branches a variation (plyOf null off mainline)', async () => {
    const store = useGamesStore()
    await openLine(store, ['e4', 'e5', 'Bc4'])

    store.first()
    store.playMove({ from: 'd2', to: 'd4' }) // not the mainline e4
    expect(store.currentId).not.toBe(1)
    expect(store.plyOf(store.currentId)).toBeNull() // off the mainline
    expect(store.tree.nodes).toHaveLength(5) // a new variation node was added
  })

  it('graftReview adds the engine line as a variation at a critical move', async () => {
    const store = useGamesStore()
    await openLine(store, ['e4', 'e5', 'Bc4', 'Nc6'])

    const review: GameReview = {
      start_fen: 'startpos',
      moves: [
        {
          ply: 4,
          san: 'Nc6',
          eval_cp: -150,
          classification: 'mistake',
          explanation: 'Bc5 was better.',
          best_line: ['Bc5'],
        },
      ],
      summary: {
        white: { acpl: 0, accuracy: 100, inaccuracies: 0, mistakes: 0, blunders: 0 },
        black: { acpl: 0, accuracy: 100, inaccuracies: 0, mistakes: 1, blunders: 0 },
      },
    }
    store.graftReview(review)

    // The Bc4 node (id 3) gains the engine's sibling line; mainline ply map holds.
    const prior = store.tree.nodes.find((n) => n.id === 3)!
    expect(prior.children).toHaveLength(2)
    expect(store.mainlinePath()).toEqual([0, 1, 2, 3, 4])
  })
})

describe('games store — PGN export', () => {
  beforeEach(() => {
    setActivePinia(createPinia())
    vi.clearAllMocks()
    vi.mocked(api.games.linkedStudies).mockResolvedValue([])
  })

  async function openGame(store: ReturnType<typeof useGamesStore>) {
    vi.mocked(api.games.get).mockResolvedValue(detail(5))
    vi.mocked(api.games.tree).mockResolvedValue(lineTree(['e4', 'e5']))
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
