import { describe, it, expect, vi, beforeEach } from 'vitest'
import { flushPromises, mount } from '@vue/test-utils'
import { setActivePinia, createPinia } from 'pinia'
import ImportView from './ImportView.vue'
import { api } from '../api.js'

vi.mock('../api.js', () => ({
  api: {
    databases: { list: vi.fn() },
    import: { sync: vi.fn(), uploadPgn: vi.fn() },
  },
}))

beforeEach(() => {
  vi.clearAllMocks()
  setActivePinia(createPinia())
  api.databases.list.mockResolvedValue([
    { id: 1, name: 'My games', kind: 'own', global: false },
    { id: 2, name: 'Masters', kind: 'master', global: true },
  ])
})

describe('ImportView', () => {
  it('populates the target picker and defaults to the first collection', async () => {
    const wrapper = mount(ImportView)
    await flushPromises()

    const options = wrapper.find('[data-test="target"]').findAll('option')
    expect(options).toHaveLength(2)
    expect(wrapper.find('[data-test="target"]').element.value).toBe('1')
  })

  it('shows an empty-state hint when there are no collections', async () => {
    api.databases.list.mockResolvedValueOnce([])
    const wrapper = mount(ImportView)
    await flushPromises()
    expect(wrapper.find('[data-test="no-databases"]').exists()).toBe(true)
  })

  it('disables the sync button until a username is entered', async () => {
    const wrapper = mount(ImportView)
    await flushPromises()

    const button = wrapper.find('[data-test="sync-submit"]')
    expect(button.attributes('disabled')).toBeDefined()

    await wrapper.find('[data-test="username"]').setValue('alice')
    expect(button.attributes('disabled')).toBeUndefined()
  })

  it('hides the token field for Chess.com (tokenless)', async () => {
    const wrapper = mount(ImportView)
    await flushPromises()

    expect(wrapper.find('[data-test="token"]').exists()).toBe(true)
    await wrapper.find('[data-test="source"]').setValue('chesscom')
    expect(wrapper.find('[data-test="token"]').exists()).toBe(false)
  })

  it('triggers a sync against the chosen collection and renders the result', async () => {
    api.import.sync.mockResolvedValue({ imported: 7 })
    const wrapper = mount(ImportView)
    await flushPromises()

    await wrapper.find('[data-test="username"]').setValue('alice')
    await wrapper.find('[data-test="token"]').setValue('tok')
    await wrapper.find('[data-test="sync-form"]').trigger('submit.prevent')
    await flushPromises()

    expect(api.import.sync).toHaveBeenCalledWith(1, 'lichess', 'alice', 'tok')
    const job = wrapper.find('[data-test="job"]')
    expect(job.find('[data-test="job-status"]').text()).toBe('success')
    expect(job.find('[data-test="job-imported"]').text()).toContain('7')
    expect(wrapper.find('[data-test="summary"]').text()).toContain('7 game(s) imported')
  })

  it('surfaces a failed sync as an error job', async () => {
    api.import.sync.mockRejectedValueOnce(new Error('no such user'))
    const wrapper = mount(ImportView)
    await flushPromises()

    await wrapper.find('[data-test="username"]').setValue('ghost')
    await wrapper.find('[data-test="sync-form"]').trigger('submit.prevent')
    await flushPromises()

    expect(wrapper.find('[data-test="job-error"]').text()).toContain('no such user')
  })

  it('uploads a selected PGN file into the chosen collection', async () => {
    api.import.uploadPgn.mockResolvedValue({ imported: 2 })
    const wrapper = mount(ImportView)
    await flushPromises()

    // Drive the file-input change handler with a fake file exposing async
    // text() (jsdom's File does not implement Blob.text()).
    const file = { name: 'games.pgn', text: () => Promise.resolve('[Event "x"]\n\n1. e4 *') }
    const input = wrapper.find('[data-test="pgn-file"]')
    Object.defineProperty(input.element, 'files', { value: [file], configurable: true })
    await input.trigger('change')

    await wrapper.find('[data-test="pgn-form"]').trigger('submit.prevent')
    await flushPromises()

    expect(api.import.uploadPgn).toHaveBeenCalledWith(1, '[Event "x"]\n\n1. e4 *')
    expect(wrapper.find('[data-test="job-imported"]').text()).toContain('2')
  })
})
