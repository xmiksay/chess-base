import { describe, it, expect, beforeEach, vi } from 'vitest'
import { flushPromises, mount } from '@vue/test-utils'
import { setActivePinia, createPinia } from 'pinia'

vi.mock('../api', () => ({
  api: {
    search: { tree: vi.fn(), games: vi.fn() },
  },
}))

import { api } from '../api'
import PositionExplorer from './PositionExplorer.vue'
import AddLineToStudyDialog from './AddLineToStudyDialog.vue'
import { useSearchStore } from '../stores/search'

const stubs = ['Board', 'AddLineToStudyDialog']

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
