import { describe, it, expect, beforeEach, vi } from 'vitest'
import { flushPromises, mount } from '@vue/test-utils'
import { setActivePinia, createPinia } from 'pinia'

vi.mock('../api', () => ({
  api: {
    health: vi.fn(),
    search: { tree: vi.fn(), games: vi.fn() },
    explorer: { masters: vi.fn() },
  },
}))

import { api } from '../api'
import PositionExplorer from './PositionExplorer.vue'
import AddLineToStudyDialog from './AddLineToStudyDialog.vue'
import { useSearchStore } from '../stores/search'

const stubs = ['Board', 'AddLineToStudyDialog', 'MastersTopGames']

async function setup() {
  const wrapper = mount(PositionExplorer, { global: { stubs } })
  await flushPromises()
  return wrapper
}

describe('PositionExplorer — Add line to study (issue #173)', () => {
  beforeEach(() => {
    setActivePinia(createPinia())
    vi.clearAllMocks()
    vi.mocked(api.search.tree).mockResolvedValue([])
    vi.mocked(api.search.games).mockResolvedValue([])
    vi.mocked(api.health).mockResolvedValue({ mode: 'local' })
  })

  it('disables the button at the start position', async () => {
    const wrapper = await setup()
    const button = wrapper.find('[data-test="add-line-to-study"]').element as HTMLButtonElement
    expect(button.disabled).toBe(true)
    expect(wrapper.findComponent(AddLineToStudyDialog).exists()).toBe(false)
  })

  it('enables the button once a line is played and opens the dialog', async () => {
    vi.mocked(api.search.tree).mockResolvedValue([
      { san: 'e5', count: 3, white: 1, draws: 1, black: 1 },
    ])
    const wrapper = await setup()
    const search = useSearchStore()
    await search.playSan('e4')
    await flushPromises()

    const button = wrapper.find('[data-test="add-line-to-study"]').element as HTMLButtonElement
    expect(button.disabled).toBe(false)

    await wrapper.find('[data-test="add-line-to-study"]').trigger('click')
    const dialog = wrapper.findComponent(AddLineToStudyDialog)
    expect(dialog.exists()).toBe(true)
    expect(dialog.props('sans')).toEqual(['e4'])
  })
})

describe('PositionExplorer — Lichess Masters source (ADR-0053)', () => {
  const report = {
    total: 6,
    white: 3,
    draws: 2,
    black: 1,
    opening: { eco: 'B20', name: 'Sicilian Defense' },
    moves: [{ san: 'c5', uci: 'c7c5', count: 6, white: 3, draws: 2, black: 1, average_rating: 2600 }],
    top_games: [],
  }

  beforeEach(() => {
    setActivePinia(createPinia())
    vi.clearAllMocks()
    vi.mocked(api.search.tree).mockResolvedValue([])
    vi.mocked(api.search.games).mockResolvedValue([])
    vi.mocked(api.explorer.masters).mockResolvedValue(report)
  })

  it('hides the source toggle when the server has no Lichess token', async () => {
    vi.mocked(api.health).mockResolvedValue({ mode: 'server', masters: false })
    const wrapper = await setup()
    expect(wrapper.find('[data-test="explorer-source"]').exists()).toBe(false)
  })

  it('switches to Masters: year-only filter, masters rows, labelled stat', async () => {
    vi.mocked(api.health).mockResolvedValue({ mode: 'server', masters: true })
    const wrapper = await setup()
    const search = useSearchStore()
    search.mastersYears.since = '1990'

    await wrapper.find('[data-test="source-masters"]').trigger('click')
    await flushPromises()

    expect(api.explorer.masters).toHaveBeenCalledWith(expect.any(String), { since: '1990' })
    expect(wrapper.find('[data-test="masters-filter"]').exists()).toBe(true)
    expect(wrapper.find('[data-test="filter-player"]').exists()).toBe(false)
    expect(wrapper.find('[data-test="masters-opening"]').text()).toContain('B20')
    expect(wrapper.findAll('[data-test="tree-row"]')).toHaveLength(1)

    await wrapper.find('[data-test="tree-row"]').trigger('click')
    await flushPromises()
    await wrapper.find('[data-test="add-line-to-study"]').trigger('click')
    const dialog = wrapper.findComponent(AddLineToStudyDialog)
    expect(dialog.props('statLabel')).toBe('Masters')
    expect(dialog.props('stat')).toMatchObject({ san: 'c5', count: 6 })
  })

  it('drops the captured stat when the source changes', async () => {
    vi.mocked(api.health).mockResolvedValue({ mode: 'server', masters: true })
    vi.mocked(api.search.tree).mockResolvedValue([{ san: 'e4', count: 2, white: 1, draws: 1, black: 0 }])
    await setup()
    const search = useSearchStore()
    await search.playSan('e4')
    expect(search.lastMoveStat).not.toBeNull()
    await search.setSource('masters')
    expect(search.lastMoveStat).toBeNull()
    expect(search.line).toEqual(['e4'])
  })
})
