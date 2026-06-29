import { describe, it, expect, vi, beforeEach } from 'vitest'
import { flushPromises, mount } from '@vue/test-utils'
import { setActivePinia, createPinia } from 'pinia'
import SettingsView from './SettingsView.vue'
import { api } from '../api'

vi.mock('../api', () => ({
  api: {
    settings: { get: vi.fn(), set: vi.fn() },
    databases: { list: vi.fn() },
    engines: {
      list: vi.fn(),
      default: vi.fn(),
      upsert: vi.fn(),
      setDefault: vi.fn(),
      remove: vi.fn(),
    },
  },
}))

beforeEach(() => {
  vi.clearAllMocks()
  window.localStorage.clear()
  setActivePinia(createPinia())
  vi.mocked(api.settings.get).mockResolvedValue({ theme: 'system', board_theme: 'brown' })
  vi.mocked(api.settings.set).mockResolvedValue({ theme: 'system', board_theme: 'blue' })
  vi.mocked(api.databases.list).mockResolvedValue([
    { id: 1, name: 'My games', owner_id: 'u1', kind: 'own', index_depth: null, global: false },
    { id: 2, name: 'Masters', owner_id: null, kind: 'master', index_depth: null, global: true },
  ])
  vi.mocked(api.engines.list).mockResolvedValue([])
  vi.mocked(api.engines.default).mockResolvedValue({ default: null })
})

describe('SettingsView', () => {
  it('loads settings and populates the default-database selector', async () => {
    const wrapper = mount(SettingsView)
    await flushPromises()

    const options = wrapper.findAll('[data-test="default-database"] option')
    // "None" + the two databases.
    expect(options).toHaveLength(3)
    expect(wrapper.text()).toContain('My games')
    expect(wrapper.text()).toContain('Masters')
  })

  it('persists a board-theme change through the store', async () => {
    const wrapper = mount(SettingsView)
    await flushPromises()

    await wrapper.find('[data-test="board-theme"]').setValue('blue')
    await flushPromises()

    expect(api.settings.set).toHaveBeenCalledWith(
      expect.objectContaining({ board_theme: 'blue' }),
    )
  })

  it('persists a default-database change as a number', async () => {
    const wrapper = mount(SettingsView)
    await flushPromises()

    await wrapper.find('[data-test="default-database"]').setValue('2')
    await flushPromises()

    expect(api.settings.set).toHaveBeenCalledWith(
      expect.objectContaining({ default_database_id: 2 }),
    )
  })
})
