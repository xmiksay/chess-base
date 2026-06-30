import { describe, it, expect, beforeEach, vi } from 'vitest'
import { setActivePinia, createPinia } from 'pinia'

// Mock the API client so the store is tested against fakes, no network.
vi.mock('../api', () => ({
  api: {
    studies: {
      list: vi.fn(),
      get: vi.fn(),
      create: vi.fn(),
      generate: vi.fn(),
      importPgn: vi.fn(),
      exportPgn: vi.fn(),
      rename: vi.fn(),
      remove: vi.fn(),
    },
  },
}))

import { api } from '../api'
import { useStudiesStore } from './studies'
import type { Study } from '../types'

describe('studies store', () => {
  beforeEach(() => {
    setActivePinia(createPinia())
    vi.clearAllMocks()
  })

  it('refresh loads the list of summaries', async () => {
    vi.mocked(api.studies.list).mockResolvedValue([
      { id: 1, database_id: 1, name: 'Sicilian', global: false, owner_id: null },
    ])
    const store = useStudiesStore()
    await store.refresh()
    expect(store.list).toHaveLength(1)
    expect(store.list[0].name).toBe('Sicilian')
    expect(store.loading).toBe(false)
  })

  it('open loads a study with its tree into current', async () => {
    const view: Study = {
      id: 2,
      database_id: 1,
      name: 'Ruy',
      global: false,
      owner_id: null,
      tree: {
        root: 0,
        nodes: [{ id: 0, parent: null, san: null, comment: null, nags: [], children: [] }],
      },
    }
    vi.mocked(api.studies.get).mockResolvedValue(view)
    const store = useStudiesStore()
    await store.open(2)
    expect(store.current).toEqual(view)
  })

  it('importPgn opens the new study and refreshes the list', async () => {
    const study: Study = {
      id: 3,
      database_id: 1,
      name: 'Imported',
      global: false,
      owner_id: null,
      tree: { root: 0, nodes: [] },
    }
    vi.mocked(api.studies.importPgn).mockResolvedValue(study)
    vi.mocked(api.studies.list).mockResolvedValue([study])
    const store = useStudiesStore()
    await store.importPgn(7, 'Imported', '1. e4 e5 *')
    expect(api.studies.importPgn).toHaveBeenCalledWith(7, 'Imported', '1. e4 e5 *', false)
    expect(store.current!.id).toBe(3)
    expect(store.list).toHaveLength(1)
  })

  it('generate calls the api and refreshes the list', async () => {
    const view = {
      id: 8,
      database_id: 2,
      name: 'Repertoire',
      global: false,
      node_count: 40,
      rejected: 3,
    }
    vi.mocked(api.studies.generate).mockResolvedValue(view)
    vi.mocked(api.studies.list).mockResolvedValue([
      { id: 8, database_id: 2, name: 'Repertoire', global: false, owner_id: null },
    ])
    const store = useStudiesStore()
    const body = { database_id: 2, name: 'Repertoire', engine_depth: 18 }
    const result = await store.generate(body)

    expect(api.studies.generate).toHaveBeenCalledWith(body)
    expect(result).toEqual(view)
    expect(store.list).toHaveLength(1)
    expect(store.list[0].id).toBe(8)
  })

  it('exportPgn returns the PGN string and defaults to keeping evals', async () => {
    vi.mocked(api.studies.exportPgn).mockResolvedValue('1. e4 e5 *')
    const store = useStudiesStore()
    await expect(store.exportPgn(3)).resolves.toBe('1. e4 e5 *')
    expect(api.studies.exportPgn).toHaveBeenCalledWith(3, { eval: true })
  })

  it('exportPgn forwards eval=false for a plain export', async () => {
    vi.mocked(api.studies.exportPgn).mockResolvedValue('1. e4 e5 *')
    const store = useStudiesStore()
    await store.exportPgn(3, false)
    expect(api.studies.exportPgn).toHaveBeenCalledWith(3, { eval: false })
  })

  it('rename keeps current and the list summary in sync', async () => {
    const store = useStudiesStore()
    store.list = [{ id: 4, database_id: 1, name: 'Old', global: false, owner_id: null }]
    store.current = {
      id: 4,
      database_id: 1,
      name: 'Old',
      global: false,
      owner_id: null,
      tree: { root: 0, nodes: [] },
    }
    vi.mocked(api.studies.rename).mockResolvedValue({
      id: 4,
      database_id: 1,
      name: 'New',
      global: false,
      owner_id: null,
      tree: { root: 0, nodes: [] },
    })
    await store.rename(4, 'New')
    expect(store.current!.name).toBe('New')
    expect(store.list[0].name).toBe('New')
  })

  it('remove drops the study and clears current when it was open', async () => {
    const store = useStudiesStore()
    store.list = [{ id: 5, database_id: 1, name: 'Gone', global: false, owner_id: null }]
    store.current = {
      id: 5,
      database_id: 1,
      name: 'Gone',
      global: false,
      owner_id: null,
      tree: { root: 0, nodes: [] },
    }
    vi.mocked(api.studies.remove).mockResolvedValue(null)
    await store.remove(5)
    expect(store.list).toHaveLength(0)
    expect(store.current).toBeNull()
  })

  it('surfaces failures on error and resets loading', async () => {
    vi.mocked(api.studies.list).mockRejectedValue(new Error('boom'))
    const store = useStudiesStore()
    await expect(store.refresh()).rejects.toThrow('boom')
    expect(store.error).toBe('boom')
    expect(store.loading).toBe(false)
  })
})
