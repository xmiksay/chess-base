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
  useRouter: () => ({ push, replace }),
  useRoute: () => route,
}))

import { api } from '../api'
import GamesView from './GamesView.vue'
import { useGamesStore } from '../stores/games'
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
  }
}

const stubs = ['Board', 'BoardControls', 'MoveTree', 'MoveComment', 'EnginePanel', 'GameReviewPanel']

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
      games.openGame = { id } as GameDetail
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
