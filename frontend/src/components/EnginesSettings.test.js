import { describe, it, expect, vi, beforeEach } from 'vitest'
import { flushPromises, mount } from '@vue/test-utils'
import EnginesSettings from './EnginesSettings.vue'
import { api } from '../api.js'

vi.mock('../api.js', () => ({
  api: {
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
  api.engines.list.mockResolvedValue([
    { name: 'Stockfish', path: '/usr/bin/stockfish' },
    { name: 'SF-Windows', path: '/opt/sf.exe', runner: '/usr/bin/wine' },
  ])
  api.engines.default.mockResolvedValue({ default: 'Stockfish', resolved: {} })
})

describe('EnginesSettings', () => {
  it('lists engines and marks the default', async () => {
    const wrapper = mount(EnginesSettings)
    await flushPromises()

    const rows = wrapper.findAll('[data-test="engine-row"]')
    expect(rows).toHaveLength(2)
    expect(wrapper.text()).toContain('Stockfish')
    expect(wrapper.text()).toContain('/usr/bin/wine') // runner shown
    // The default engine's radio is checked.
    const checked = rows[0].find('input[type="radio"]').element
    expect(checked.checked).toBe(true)
  })

  it('upserts a new engine including the runner field', async () => {
    api.engines.upsert.mockResolvedValue(null)
    const wrapper = mount(EnginesSettings)
    await flushPromises()

    await wrapper.find('input[placeholder="Name (e.g. Stockfish 16)"]').setValue('Maia')
    await wrapper.find('input[placeholder="Binary path"]').setValue('/usr/bin/lc0')
    await wrapper.find('input[placeholder="Runner (optional, e.g. wine)"]').setValue('/run.sh')
    await wrapper.find('form').trigger('submit.prevent')
    await flushPromises()

    expect(api.engines.upsert).toHaveBeenCalledWith({
      name: 'Maia',
      path: '/usr/bin/lc0',
      runner: '/run.sh',
    })
  })

  it('selecting a radio sets that engine as the default', async () => {
    api.engines.setDefault.mockResolvedValue(null)
    const wrapper = mount(EnginesSettings)
    await flushPromises()

    const rows = wrapper.findAll('[data-test="engine-row"]')
    await rows[1].find('input[type="radio"]').trigger('change')
    expect(api.engines.setDefault).toHaveBeenCalledWith('SF-Windows')
  })

  it('surfaces an API error message', async () => {
    api.engines.list.mockRejectedValueOnce(new Error('admin privileges required'))
    const wrapper = mount(EnginesSettings)
    await flushPromises()
    expect(wrapper.find('[data-test="error"]').text()).toContain('admin privileges required')
  })
})
