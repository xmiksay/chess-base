import { describe, it, expect, beforeEach, vi } from 'vitest'
import { setActivePinia, createPinia } from 'pinia'

// Mock the API client so the store is tested against fakes, no network.
vi.mock('../api', () => ({
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

import { api } from '../api'
import { useCollectionsStore } from './collections'
import type { Database } from '../types'

describe('collections store', () => {
  beforeEach(() => {
    setActivePinia(createPinia())
    vi.clearAllMocks()
    vi.mocked(api.whoami).mockResolvedValue({ id: 'u1', is_admin: false })
  })

  it('refresh loads the visible databases and the admin flag', async () => {
    vi.mocked(api.whoami).mockResolvedValue({ id: 'u1', is_admin: true })
    vi.mocked(api.databases.list).mockResolvedValue([
      { id: 1, owner_id: 'u1', name: 'Mine', kind: 'own', index_depth: null, global: false },
      { id: 2, owner_id: null, name: 'Masters', kind: 'master', index_depth: null, global: true },
    ])
    const store = useCollectionsStore()
    await store.refresh()
    expect(store.list).toHaveLength(2)
    expect(store.isAdmin).toBe(true)
    expect(store.loading).toBe(false)
  })

  it('canWrite gates global databases behind admin', async () => {
    const store = useCollectionsStore()
    store.isAdmin = false
    expect(store.canWrite({ global: false } as Database)).toBe(true)
    expect(store.canWrite({ global: true } as Database)).toBe(false)
    store.isAdmin = true
    expect(store.canWrite({ global: true } as Database)).toBe(true)
  })

  it('create appends the new database to the list', async () => {
    const db: Database = {
      id: 3,
      owner_id: 'u1',
      name: 'Repertoire',
      kind: 'own',
      index_depth: null,
      global: false,
    }
    vi.mocked(api.databases.create).mockResolvedValue(db)
    const store = useCollectionsStore()
    await store.create('Repertoire', 'own')
    expect(api.databases.create).toHaveBeenCalledWith('Repertoire', 'own', false)
    expect(store.list).toHaveLength(1)
    expect(store.list[0]).toEqual(db)
  })

  it('rename keeps the list summary in sync', async () => {
    const store = useCollectionsStore()
    store.list = [
      { id: 4, owner_id: 'u1', name: 'Old', kind: 'own', index_depth: null, global: false },
    ]
    vi.mocked(api.databases.rename).mockResolvedValue({
      id: 4,
      owner_id: 'u1',
      name: 'New',
      kind: 'own',
      index_depth: null,
      global: false,
    })
    await store.rename(4, 'New')
    expect(store.list[0].name).toBe('New')
  })

  it('remove drops the database from the list', async () => {
    const store = useCollectionsStore()
    store.list = [
      { id: 5, owner_id: 'u1', name: 'Gone', kind: 'own', index_depth: null, global: false },
    ]
    vi.mocked(api.databases.remove).mockResolvedValue(null)
    await store.remove(5)
    expect(store.list).toHaveLength(0)
  })

  it('surfaces failures on error and resets loading', async () => {
    vi.mocked(api.databases.list).mockRejectedValue(new Error('boom'))
    const store = useCollectionsStore()
    await expect(store.refresh()).rejects.toThrow('boom')
    expect(store.error).toBe('boom')
    expect(store.loading).toBe(false)
  })
})
