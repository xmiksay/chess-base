import { describe, it, expect, vi, beforeEach } from 'vitest'
import { flushPromises, mount } from '@vue/test-utils'
import { setActivePinia, createPinia } from 'pinia'
import SettingsView from './SettingsView.vue'
import { api } from '../api.js'

vi.mock('../api.js', () => ({
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
  api.settings.get.mockResolvedValue({ theme: 'system', board_theme: 'brown' })
  api.settings.set.mockResolvedValue({ theme: 'system', board_theme: 'blue' })
  api.databases.list.mockResolvedValue([
    { id: 1, name: 'My games' },
    { id: 2, name: 'Masters' },
  ])
  api.engines.list.mockResolvedValue([])
  api.engines.default.mockResolvedValue({ default: null, resolved: null })
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
