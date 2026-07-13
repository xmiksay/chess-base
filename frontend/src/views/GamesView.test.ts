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

const push = vi.fn()
vi.mock('vue-router', () => ({ useRouter: () => ({ push }) }))

import { api } from '../api'
import GamesView from './GamesView.vue'
import { useGamesStore } from '../stores/games'
import type { GameRow, Study } from '../types'

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
