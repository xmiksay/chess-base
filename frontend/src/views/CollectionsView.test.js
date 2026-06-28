import { describe, it, expect, vi, beforeEach } from 'vitest'
import { flushPromises, mount } from '@vue/test-utils'
import { setActivePinia, createPinia } from 'pinia'
import CollectionsView from './CollectionsView.vue'
import { api } from '../api.js'

vi.mock('../api.js', () => ({
  api: {
    whoami: vi.fn(),
    databases: {
      list: vi.fn(),
      create: vi.fn(),
      rename: vi.fn(),
      remove: vi.fn(),
    },
  },
}))

beforeEach(() => {
  vi.clearAllMocks()
  setActivePinia(createPinia())
  api.whoami.mockResolvedValue({ id: 'u1', is_admin: false })
  api.databases.list.mockResolvedValue([
    { id: 1, name: 'My games', kind: 'own', global: false },
    { id: 2, name: 'Masters', kind: 'master', global: true },
  ])
})

describe('CollectionsView', () => {
  it('lists databases with an ownership badge', async () => {
    const wrapper = mount(CollectionsView)
    await flushPromises()

    const rows = wrapper.findAll('[data-test="db-row"]')
    expect(rows).toHaveLength(2)
    const badges = wrapper.findAll('[data-test="badge"]').map((b) => b.text())
    expect(badges).toEqual(['Mine', 'Global'])
  })

  it('renders global databases read-only for a non-admin', async () => {
    const wrapper = mount(CollectionsView)
    await flushPromises()

    const rows = wrapper.findAll('[data-test="db-row"]')
    // Own database is writable; the global one is read-only.
    expect(rows[0].find('[data-test="delete"]').exists()).toBe(true)
    expect(rows[1].find('[data-test="delete"]').exists()).toBe(false)
    expect(rows[1].find('[data-test="readonly"]').exists()).toBe(true)
    // The global-create checkbox is hidden for non-admins.
    expect(wrapper.find('[data-test="global"]').exists()).toBe(false)
  })

  it('creates a database from the form', async () => {
    api.databases.create.mockResolvedValue({
      id: 3,
      name: 'Repertoire',
      kind: 'own',
      global: false,
    })
    const wrapper = mount(CollectionsView)
    await flushPromises()

    await wrapper.find('input[placeholder="New collection name"]').setValue('Repertoire')
    await wrapper.find('[data-test="create-form"]').trigger('submit.prevent')
    await flushPromises()

    expect(api.databases.create).toHaveBeenCalledWith('Repertoire', 'own', false)
    expect(wrapper.findAll('[data-test="db-row"]')).toHaveLength(3)
  })

  it('renames a database inline', async () => {
    api.databases.rename.mockResolvedValue({
      id: 1,
      name: 'Renamed',
      kind: 'own',
      global: false,
    })
    const wrapper = mount(CollectionsView)
    await flushPromises()

    await wrapper.findAll('[data-test="db-row"]')[0].find('[data-test="rename"]').trigger('click')
    await wrapper.find('input[aria-label="New name"]').setValue('Renamed')
    await wrapper.find('[data-test="save"]').trigger('click')
    await flushPromises()

    expect(api.databases.rename).toHaveBeenCalledWith(1, 'Renamed')
    expect(wrapper.findAll('[data-test="db-row"]')[0].text()).toContain('Renamed')
  })

  it('deletes a database', async () => {
    api.databases.remove.mockResolvedValue(null)
    const wrapper = mount(CollectionsView)
    await flushPromises()

    await wrapper.findAll('[data-test="db-row"]')[0].find('[data-test="delete"]').trigger('click')
    await flushPromises()

    expect(api.databases.remove).toHaveBeenCalledWith(1)
    expect(wrapper.findAll('[data-test="db-row"]')).toHaveLength(1)
  })

  it('shows the global checkbox and creates a global database for an admin', async () => {
    api.whoami.mockResolvedValue({ id: 'admin', is_admin: true })
    api.databases.create.mockResolvedValue({
      id: 9,
      name: 'World',
      kind: 'master',
      global: true,
    })
    const wrapper = mount(CollectionsView)
    await flushPromises()

    expect(wrapper.find('[data-test="global"]').exists()).toBe(true)
    await wrapper.find('input[placeholder="New collection name"]').setValue('World')
    await wrapper.find('select[aria-label="Kind"]').setValue('master')
    await wrapper.find('[data-test="global"]').setValue(true)
    await wrapper.find('[data-test="create-form"]').trigger('submit.prevent')
    await flushPromises()

    expect(api.databases.create).toHaveBeenCalledWith('World', 'master', true)
  })

  it('surfaces an API error message', async () => {
    api.databases.list.mockRejectedValueOnce(new Error('nope'))
    const wrapper = mount(CollectionsView)
    await flushPromises()
    expect(wrapper.find('[data-test="error"]').text()).toContain('nope')
  })
})
