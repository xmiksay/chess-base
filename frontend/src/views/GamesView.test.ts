import { describe, it, expect, beforeEach, vi } from 'vitest'
import { flushPromises, mount } from '@vue/test-utils'
import { setActivePinia, createPinia } from 'pinia'

// The merge selection (issue #170) drives api.studies.mergeGames; onMounted also
// probes health + databases. Stub the rest of the client so nothing hits network.
vi.mock('../api', () => ({
  api: {
    health: vi.fn().mockResolvedValue({ engine: false }),
    databases: { list: vi.fn().mockResolvedValue([]) },
    studies: { mergeGames: vi.fn() },
  },
}))

// Router mock (issue #212): a mutable route object each test seeds before
// mount, plus push/replace spies for asserting the mirrored navigation.
const { push, replace, route } = vi.hoisted(() => ({
  push: vi.fn(),
  replace: vi.fn(),
  route: {
    name: 'games' as string,
    params: {} as Record<string, string>,
    query: {} as Record<string, string>,
  },
}))
vi.mock('vue-router', () => ({
  useRouter: () => ({ push, replace, resolve: (to: { params?: { id?: string } }) => ({ href: `/games/${to.params?.id}` }) }),
  useRoute: () => route,
}))

import { api } from '../api'
import GamesView from './GamesView.vue'
import Board from '../components/Board.vue'
import EnginePanel from '../components/EnginePanel.vue'
import GameReviewPanel from '../components/GameReviewPanel.vue'
import { useGamesStore } from '../stores/games'
import { useAuthStore } from '../stores/auth'
import type { Database, GameDetail, GameRow, Study } from '../types'

function row(id: number, white: string, black: string): GameRow {
  return {
    id,
    white,
    black,
    result: '1-0',
    date: '2023.01.01',
    eco: null,
    white_elo: null,
    black_elo: null,
    public: false,
  }
}

// ShareToggle is intentionally left un-stubbed (real component, no network) so
// the sharing tests below can assert on its rendered checkbox/link.
const stubs = {
  Board: true,
  BoardControls: true,
  MoveTree: true,
  MoveComment: true,
  EnginePanel: true,
  GameReviewPanel: true,
}

function openedGame(id: number, isPublic = false): GameDetail {
  return {
    id,
    white: 'Carlsen',
    black: 'Nepo',
    result: '1-0',
    date: null,
    eco: null,
    white_elo: null,
    black_elo: null,
    public: isPublic,
    pgn: '',
  }
}

async function setup() {
  const games = useGamesStore()
  games.games = [row(1, 'Carlsen', 'Nepo'), row(2, 'Carlsen', 'So'), row(3, 'Carlsen', 'Ding')]
  games.total = 3
  const wrapper = mount(GamesView, { global: { stubs } })
  await flushPromises()
  return { games, wrapper }
}

describe('GamesView merge selection', () => {
  beforeEach(() => {
    setActivePinia(createPinia())
    vi.clearAllMocks()
    route.name = 'games'
    route.params = {}
    route.query = {}
    vi.mocked(api.health).mockResolvedValue({ mode: 'server', engine: false })
    vi.mocked(api.databases.list).mockResolvedValue([])
  })

  it('shows the merge bar only once games are ticked and needs at least two', async () => {
    const { wrapper } = await setup()
    // No selection → no merge bar.
    expect(wrapper.find('[data-test="merge-games"]').exists()).toBe(false)

    // Ticking one game shows the bar but keeps merge disabled (need ≥2).
    await wrapper.findAll('[data-test="select-game"]')[0].trigger('change')
    const merge = wrapper.find('[data-test="merge-games"]')
    expect(merge.exists()).toBe(true)
    expect((merge.element as HTMLButtonElement).disabled).toBe(true)

    // A second tick enables it.
    await wrapper.findAll('[data-test="select-game"]')[1].trigger('change')
    expect((wrapper.find('[data-test="merge-games"]').element as HTMLButtonElement).disabled).toBe(
      false,
    )
  })

  it('merges the selected games and routes to the study editor', async () => {
    const merged = { id: 9, name: 'Repertoire' } as Study
    vi.mocked(api.studies.mergeGames).mockResolvedValue(merged)
    const promptSpy = vi.spyOn(window, 'prompt').mockReturnValue('Repertoire')

    const { wrapper } = await setup()
    await wrapper.findAll('[data-test="select-game"]')[0].trigger('change')
    await wrapper.findAll('[data-test="select-game"]')[2].trigger('change')
    await wrapper.find('[data-test="merge-games"]').trigger('click')
    await flushPromises()

    expect(api.studies.mergeGames).toHaveBeenCalledWith({
      game_ids: [1, 3],
      name: 'Repertoire',
    })
    expect(push).toHaveBeenCalledWith({ name: 'studies' })
    // The selection clears after a successful merge.
    expect(wrapper.find('[data-test="merge-games"]').exists()).toBe(false)
    promptSpy.mockRestore()
  })
})

// Issue #212: /games/:id? + ?db= — hydrate from the URL, mirror selection back.
describe('GamesView URL addressability', () => {
  const db = (id: number): Database => ({
    id,
    owner_id: null,
    name: `DB ${id}`,
    kind: 'own',
    index_depth: null,
    global: false,
  })

  beforeEach(() => {
    setActivePinia(createPinia())
    vi.clearAllMocks()
    route.name = 'games'
    route.params = {}
    route.query = {}
    vi.mocked(api.health).mockResolvedValue({ mode: 'server', engine: false })
    vi.mocked(api.databases.list).mockResolvedValue([db(1), db(2)])
  })

  it('hydrates the database and game from /games/5?db=2 on mount', async () => {
    route.params = { id: '5' }
    route.query = { db: '2' }
    const games = useGamesStore()
    const selectDatabase = vi.spyOn(games, 'selectDatabase').mockResolvedValue(undefined)
    const open = vi.spyOn(games, 'open').mockResolvedValue(undefined)

    mount(GamesView, { global: { stubs } })
    await flushPromises()

    // The URL's ?db= wins over the settings default (which would be DB 1).
    expect(selectDatabase).toHaveBeenCalledWith(2)
    expect(open).toHaveBeenCalledWith(5)
  })

  it('mirrors an opened game into the URL via router.replace', async () => {
    const games = useGamesStore()
    vi.spyOn(games, 'selectDatabase').mockResolvedValue(undefined)
    vi.spyOn(games, 'open').mockImplementation(async (id: number) => {
      games.openGame = { id, public: false } as GameDetail
    })
    games.games = [row(3, 'Carlsen', 'Ding')]
    games.total = 1

    const wrapper = mount(GamesView, { global: { stubs } })
    await flushPromises()
    replace.mockClear()

    await wrapper.find('tbody tr').trigger('click')
    await flushPromises()

    expect(replace).toHaveBeenCalledWith(
      expect.objectContaining({ name: 'games', params: { id: '3' } }),
    )
  })

  it('mirrors the selected database into ?db=', async () => {
    const games = useGamesStore()
    vi.spyOn(games, 'selectDatabase').mockImplementation(async (id: number) => {
      games.databaseId = id
    })

    const wrapper = mount(GamesView, { global: { stubs } })
    await flushPromises()
    replace.mockClear()

    await wrapper.find('select[aria-label="Database"]').setValue(2)
    await flushPromises()

    expect(replace).toHaveBeenCalledWith(
      expect.objectContaining({ name: 'games', query: { db: '2' } }),
    )
  })
})

// Issue #213, ADR-0045: the ShareToggle on an authenticated caller's header.
describe('GamesView sharing', () => {
  beforeEach(() => {
    setActivePinia(createPinia())
    vi.clearAllMocks()
    route.name = 'games'
    route.params = {}
    route.query = {}
    vi.mocked(api.health).mockResolvedValue({ mode: 'server', engine: false })
    vi.mocked(api.databases.list).mockResolvedValue([])
  })

  it('renders the public toggle for an open game, reflecting its current flag', async () => {
    const { games, wrapper } = await setup()
    games.openGame = openedGame(3)
    await flushPromises()

    const toggle = wrapper.find('[data-test="public-toggle"]')
    expect(toggle.exists()).toBe(true)
    expect((toggle.element as HTMLInputElement).checked).toBe(false)
    // Private ⇒ no link/copy affordance yet.
    expect(wrapper.find('[data-test="share-link"]').exists()).toBe(false)
  })

  it('shows the share link once the game is public', async () => {
    const { games, wrapper } = await setup()
    games.openGame = openedGame(3, true)
    await flushPromises()

    expect((wrapper.find('[data-test="public-toggle"]').element as HTMLInputElement).checked).toBe(true)
    expect((wrapper.find('[data-test="share-link"]').element as HTMLInputElement).value).toContain('/games/3')
  })
})

// Issue #213: the router only lets an anonymous caller through with a
// `/games/:id` deep link — the view must open that game directly and render
// it read-only, without touching the authenticated list/browse endpoints.
describe('GamesView anonymous read-only', () => {
  beforeEach(() => {
    setActivePinia(createPinia())
    vi.clearAllMocks()
    route.name = 'games'
    route.params = { id: '7' }
    route.query = {}
    vi.mocked(api.health).mockResolvedValue({ mode: 'server', engine: false })
  })

  it('opens the deep-linked game directly, skipping the authenticated list endpoints', async () => {
    useAuthStore().mode = 'server'
    const games = useGamesStore()
    const open = vi.spyOn(games, 'open').mockResolvedValue(undefined)

    mount(GamesView, { global: { stubs } })
    await flushPromises()

    expect(open).toHaveBeenCalledWith(7)
    expect(api.databases.list).not.toHaveBeenCalled()
  })

  it('hides the browse/write UI and renders the board read-only', async () => {
    useAuthStore().mode = 'server'
    const games = useGamesStore()
    vi.spyOn(games, 'open').mockImplementation(async (id: number) => {
      games.openGame = openedGame(id, true)
    })

    const wrapper = mount(GamesView, { global: { stubs } })
    await flushPromises()

    expect(wrapper.find('select[aria-label="Database"]').exists()).toBe(false)
    expect(wrapper.find('table').exists()).toBe(false)
    expect(wrapper.find('[data-test="public-toggle"]').exists()).toBe(false)
    expect(wrapper.findComponent(GameReviewPanel).exists()).toBe(false)
    expect(wrapper.findComponent(EnginePanel).exists()).toBe(false)
    expect(wrapper.findComponent(Board).props('movable')).toBe(false)
    expect(wrapper.find('[data-test="export-game"]').exists()).toBe(true)
  })
})
