import { describe, it, expect, beforeEach, vi } from 'vitest'
import { flushPromises, mount } from '@vue/test-utils'
import { setActivePinia, createPinia } from 'pinia'

// Mock the API client: the form drives games.saveAsStudy + folders.list.
vi.mock('../api', () => ({
  api: {
    games: { saveAsStudy: vi.fn(), linkedStudies: vi.fn() },
    folders: { list: vi.fn() },
  },
}))

import { api } from '../api'
import SaveAsAnalysisForm from './SaveAsAnalysisForm.vue'
import { useGamesStore } from '../stores/games'
import type { GameDetail, StudySummary } from '../types'

const openGame = (): GameDetail => ({
  id: 5,
  white: 'Alice',
  black: 'Bob',
  result: null,
  date: null,
  eco: null,
  white_elo: null,
  black_elo: null,
  pgn: '',
})

const savedStudy: StudySummary = {
  id: 9,
  database_id: 1,
  name: 'Alice – Bob',
  global: false,
  owner_id: null,
  folder_id: null,
  origin_game_id: 5,
}

async function setup(engineEnabled = true) {
  const games = useGamesStore()
  games.openGame = openGame()
  const wrapper = mount(SaveAsAnalysisForm, { props: { engineEnabled } })
  await flushPromises()
  await wrapper.find('[data-test="save-as-analysis"]').trigger('click')
  await flushPromises()
  return { games, wrapper }
}

describe('SaveAsAnalysisForm', () => {
  beforeEach(() => {
    setActivePinia(createPinia())
    vi.clearAllMocks()
    vi.mocked(api.folders.list).mockResolvedValue([])
    vi.mocked(api.games.linkedStudies).mockResolvedValue([])
    vi.mocked(api.games.saveAsStudy).mockResolvedValue(savedStudy)
  })

  it('defaults the name from the players', async () => {
    const { wrapper } = await setup()
    const input = wrapper.find('[data-test="save-as-analysis-name"]')
      .element as HTMLInputElement
    expect(input.value).toBe('Alice – Bob')
  })

  it('submits the form body and refreshes the linked analyses', async () => {
    vi.mocked(api.games.linkedStudies).mockResolvedValue([savedStudy])
    const { games, wrapper } = await setup()

    await wrapper.find('[data-test="save-as-analysis-name"]').setValue('My analysis')
    await wrapper.find('[data-test="save-as-analysis-analyse"]').setValue(true)
    await wrapper.find('[data-test="save-as-analysis-submit"]').trigger('click')
    await flushPromises()

    expect(api.games.saveAsStudy).toHaveBeenCalledWith(5, {
      name: 'My analysis',
      folder_id: null,
      analyse: true,
    })
    // Linked-analyses list refreshed after the save.
    expect(api.games.linkedStudies).toHaveBeenCalledWith(5)
    expect(games.linkedStudies).toHaveLength(1)
  })

  it('disables the engine checkbox when no engine is available', async () => {
    const { wrapper } = await setup(false)
    const cb = wrapper.find('[data-test="save-as-analysis-analyse"]')
      .element as HTMLInputElement
    expect(cb.disabled).toBe(true)
  })

  it('surfaces a save failure (e.g. the 503 without an engine)', async () => {
    vi.mocked(api.games.saveAsStudy).mockRejectedValue(new Error('no engine configured'))
    const { wrapper } = await setup()
    await wrapper.find('[data-test="save-as-analysis-analyse"]').setValue(true)
    await wrapper.find('[data-test="save-as-analysis-submit"]').trigger('click')
    await flushPromises()
    expect(wrapper.find('[data-test="save-as-analysis-error"]').text()).toContain(
      'no engine configured',
    )
  })
})
