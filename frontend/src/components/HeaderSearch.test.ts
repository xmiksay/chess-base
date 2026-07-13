import { describe, it, expect, beforeEach, vi } from 'vitest'
import { flushPromises, mount } from '@vue/test-utils'
import { setActivePinia, createPinia } from 'pinia'

// Bulk actions (issue #171) drive api.games.exportSelected/remove and
// api.studies.mergeGames (inside the stubbed MergeGamesDialog child); the search
// itself never runs a real fetch, so `headers` is stubbed but unused per test.
vi.mock('../api', () => ({
  api: {
    search: { headers: vi.fn() },
    games: { exportSelected: vi.fn(), remove: vi.fn() },
  },
}))

vi.mock('../lib/download', () => ({ downloadText: vi.fn() }))

const push = vi.fn()
vi.mock('vue-router', () => ({ useRouter: () => ({ push }) }))

import { api } from '../api'
import { downloadText } from '../lib/download'
import HeaderSearch from './HeaderSearch.vue'
import { useSearchStore } from '../stores/search'
import type { GameRow } from '../types'

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

function setup() {
  const search = useSearchStore()
  search.results = [row(1, 'Carlsen', 'Nepo'), row(2, 'Carlsen', 'So'), row(3, 'Carlsen', 'Ding')]
  search.searched = true
  const wrapper = mount(HeaderSearch, {
    global: { stubs: { MergeGamesDialog: true } },
  })
  return { search, wrapper }
}

describe('HeaderSearch multi-select + bulk actions', () => {
  beforeEach(() => {
    setActivePinia(createPinia())
    vi.clearAllMocks()
  })

  it('shows the bulk toolbar only once a row is ticked', async () => {
    const { wrapper } = setup()
    expect(wrapper.find('[data-test="merge-selected"]').exists()).toBe(false)

    await wrapper.findAll('[data-test="select-game"]')[0].setValue(true)
    expect(wrapper.find('[data-test="merge-selected"]').exists()).toBe(true)
  })

  it('select-all ticks every loaded row, and toggles off when all are selected', async () => {
    const { wrapper } = setup()
    const selectAll = wrapper.find('[data-test="select-all"]')

    await selectAll.setValue(true)
    const boxes = wrapper.findAll('[data-test="select-game"]')
    for (const box of boxes) {
      expect((box.element as HTMLInputElement).checked).toBe(true)
    }
    expect(wrapper.text()).toContain('3 selected')

    await selectAll.setValue(false)
    expect(wrapper.find('[data-test="merge-selected"]').exists()).toBe(false)
  })

  it('exports the selected games as one PGN download', async () => {
    vi.mocked(api.games.exportSelected).mockResolvedValue('[White "A"]\n\n1. e4 *')

    const { wrapper } = setup()
    await wrapper.findAll('[data-test="select-game"]')[0].setValue(true)
    await wrapper.findAll('[data-test="select-game"]')[1].setValue(true)
    await wrapper.find('[data-test="export-selected"]').trigger('click')
    await flushPromises()

    expect(api.games.exportSelected).toHaveBeenCalledWith([1, 2])
    expect(downloadText).toHaveBeenCalledWith('games-export.pgn', '[White "A"]\n\n1. e4 *')
  })

  it('deletes the selected games and drops them from the results', async () => {
    vi.spyOn(window, 'confirm').mockReturnValue(true)
    vi.mocked(api.games.remove).mockResolvedValue(null)

    const { search, wrapper } = setup()
    await wrapper.findAll('[data-test="select-game"]')[0].setValue(true)
    await wrapper.findAll('[data-test="select-game"]')[2].setValue(true)
    await wrapper.find('[data-test="delete-selected"]').trigger('click')
    await flushPromises()

    expect(api.games.remove).toHaveBeenCalledWith(1)
    expect(api.games.remove).toHaveBeenCalledWith(3)
    expect(search.results.map((g) => g.id)).toEqual([2])
    // The selection clears once every delete succeeds.
    expect(wrapper.find('[data-test="merge-selected"]').exists()).toBe(false)
  })

  it('keeps a failed delete selected and surfaces an error', async () => {
    vi.spyOn(window, 'confirm').mockReturnValue(true)
    vi.mocked(api.games.remove).mockRejectedValueOnce(new Error('/api/games/1 → 403'))

    const { search, wrapper } = setup()
    await wrapper.findAll('[data-test="select-game"]')[0].setValue(true)
    await wrapper.find('[data-test="delete-selected"]').trigger('click')
    await flushPromises()

    expect(search.results.map((g) => g.id)).toEqual([1, 2, 3])
    expect(wrapper.text()).toContain('Failed to delete 1 game')
  })

  it('keeps "Load more" available after a bulk delete empties the loaded page', async () => {
    vi.spyOn(window, 'confirm').mockReturnValue(true)
    vi.mocked(api.games.remove).mockResolvedValue(null)

    const { search, wrapper } = setup()
    search.nextCursor = 'more-results'
    await wrapper.findAll('[data-test="select-game"]')[0].setValue(true)
    await wrapper.findAll('[data-test="select-game"]')[1].setValue(true)
    await wrapper.findAll('[data-test="select-game"]')[2].setValue(true)
    await wrapper.find('[data-test="delete-selected"]').trigger('click')
    await flushPromises()

    expect(search.results).toHaveLength(0)
    // Every loaded row is gone, but the store still has more pages — the
    // "Load more" button must survive, not vanish with the (now-empty) table.
    expect(wrapper.find('[data-test="load-more"]').exists()).toBe(true)
    expect(wrapper.text()).not.toContain('No games match.')
  })

  it('clearing the query form also drops a stale selection and its error', async () => {
    vi.mocked(api.games.exportSelected).mockRejectedValue(new Error('boom'))

    const { wrapper } = setup()
    await wrapper.findAll('[data-test="select-game"]')[0].setValue(true)
    await wrapper.find('[data-test="export-selected"]').trigger('click')
    await flushPromises()
    expect(wrapper.text()).toContain('boom')

    await wrapper.find('form button[type="button"]').trigger('click')

    // The stale selection (and its toolbar + error) is gone, not left
    // pointing at ids that just disappeared from the (now-reset) results.
    expect(wrapper.find('[data-test="merge-selected"]').exists()).toBe(false)
    expect(wrapper.text()).not.toContain('boom')
  })

  it('clears a stale bulk error once the selection changes', async () => {
    vi.mocked(api.games.exportSelected).mockRejectedValueOnce(new Error('boom'))

    const { wrapper } = setup()
    await wrapper.findAll('[data-test="select-game"]')[0].setValue(true)
    await wrapper.find('[data-test="export-selected"]').trigger('click')
    await flushPromises()
    expect(wrapper.text()).toContain('boom')

    // Ticking a different (unrelated) row shouldn't keep showing the old error.
    await wrapper.findAll('[data-test="select-game"]')[1].setValue(true)
    expect(wrapper.text()).not.toContain('boom')
  })

  it('opens the merge dialog and clears the selection + routes to studies on success', async () => {
    const { wrapper } = setup()
    await wrapper.findAll('[data-test="select-game"]')[0].setValue(true)
    await wrapper.findAll('[data-test="select-game"]')[1].setValue(true)
    await wrapper.find('[data-test="merge-selected"]').trigger('click')

    const dialog = wrapper.findComponent({ name: 'MergeGamesDialog' })
    expect(dialog.exists()).toBe(true)
    expect(dialog.props('gameIds')).toEqual([1, 2])

    dialog.vm.$emit('merged', { id: 9, name: 'Repertoire' })
    await flushPromises()

    expect(push).toHaveBeenCalledWith({ name: 'studies' })
    expect(wrapper.find('[data-test="merge-selected"]').exists()).toBe(false)
  })
})
