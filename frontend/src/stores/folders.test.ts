import { describe, it, expect, beforeEach, vi } from 'vitest'
import { setActivePinia, createPinia } from 'pinia'

// Mock the API client so the store is tested against fakes, no network.
vi.mock('../api', () => ({
  api: {
    folders: {
      list: vi.fn(),
      create: vi.fn(),
      rename: vi.fn(),
      move: vi.fn(),
      remove: vi.fn(),
    },
  },
}))

import { api } from '../api'
import { useFoldersStore } from './folders'
import type { FolderSummary } from '../types'

const folder = (id: number, parent_id: number | null = null, name = `F${id}`): FolderSummary => ({
  id,
  owner_id: null,
  parent_id,
  name,
  global: false,
})

describe('folders store', () => {
  beforeEach(() => {
    setActivePinia(createPinia())
    vi.clearAllMocks()
  })

  it('refresh loads the folder list', async () => {
    vi.mocked(api.folders.list).mockResolvedValue([folder(1), folder(2)])
    const store = useFoldersStore()
    await store.refresh()
    expect(store.list).toHaveLength(2)
    expect(store.loading).toBe(false)
  })

  it('create posts under the parent and refreshes', async () => {
    const created = folder(3, 1, 'Child')
    vi.mocked(api.folders.create).mockResolvedValue(created)
    vi.mocked(api.folders.list).mockResolvedValue([folder(1), created])
    const store = useFoldersStore()
    const out = await store.create('Child', 1)
    expect(api.folders.create).toHaveBeenCalledWith('Child', 1)
    expect(out).toEqual(created)
    expect(store.list).toHaveLength(2)
  })

  it('rename updates the row in place', async () => {
    const store = useFoldersStore()
    store.list = [folder(4, null, 'Old')]
    vi.mocked(api.folders.rename).mockResolvedValue(folder(4, null, 'New'))
    await store.rename(4, 'New')
    expect(api.folders.rename).toHaveBeenCalledWith(4, 'New')
    expect(store.list[0].name).toBe('New')
  })

  it('move reparents and refreshes', async () => {
    vi.mocked(api.folders.move).mockResolvedValue(folder(2, 1))
    vi.mocked(api.folders.list).mockResolvedValue([folder(1), folder(2, 1)])
    const store = useFoldersStore()
    await store.move(2, 1)
    expect(api.folders.move).toHaveBeenCalledWith(2, 1)
    expect(store.list.find((f) => f.id === 2)?.parent_id).toBe(1)
  })

  it('remove deletes and refreshes', async () => {
    vi.mocked(api.folders.remove).mockResolvedValue(null)
    vi.mocked(api.folders.list).mockResolvedValue([folder(1)])
    const store = useFoldersStore()
    store.list = [folder(1), folder(2)]
    await store.remove(2)
    expect(api.folders.remove).toHaveBeenCalledWith(2)
    expect(store.list).toHaveLength(1)
  })

  it('childrenOf filters by parent_id (null ⇒ root folders)', async () => {
    const store = useFoldersStore()
    store.list = [folder(1, null), folder(2, null), folder(3, 1), folder(4, 1)]
    expect(store.childrenOf(null).map((f) => f.id)).toEqual([1, 2])
    expect(store.childrenOf(1).map((f) => f.id)).toEqual([3, 4])
    expect(store.childrenOf(99)).toEqual([])
  })

  it('surfaces failures on error and resets loading', async () => {
    vi.mocked(api.folders.list).mockRejectedValue(new Error('boom'))
    const store = useFoldersStore()
    await expect(store.refresh()).rejects.toThrow('boom')
    expect(store.error).toBe('boom')
    expect(store.loading).toBe(false)
  })
})
