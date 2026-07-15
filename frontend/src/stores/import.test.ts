import { describe, it, expect, vi, beforeEach } from 'vitest'
import { flushPromises } from '@vue/test-utils'
import { setActivePinia, createPinia } from 'pinia'
import { foldStatus, useImportStore } from './import'
import { api } from '../api'

vi.mock('../api', () => ({
  api: {
    databases: { list: vi.fn() },
    import: { sync: vi.fn(), uploadPgn: vi.fn() },
  },
}))

beforeEach(() => {
  vi.clearAllMocks()
  setActivePinia(createPinia())
})

describe('foldStatus', () => {
  it('is idle with no jobs', () => {
    expect(foldStatus([])).toMatchObject({ state: 'idle', total: 0, imported: 0 })
  })

  it('is running while any job is in flight', () => {
    const s = foldStatus([
      { id: 1, kind: 'sync', label: 'a', status: 'running', imported: 0, duplicates: 0, error: null },
      { id: 2, kind: 'sync', label: 'b', status: 'success', imported: 5, duplicates: 0, error: null },
    ])
    expect(s.state).toBe('running')
    expect(s.running).toBe(1)
  })

  it('is done when every finished job succeeded and sums the import counts', () => {
    const s = foldStatus([
      { id: 1, kind: 'sync', label: 'a', status: 'success', imported: 5, duplicates: 0, error: null },
      { id: 2, kind: 'sync', label: 'b', status: 'success', imported: 3, duplicates: 2, error: null },
    ])
    expect(s).toMatchObject({ state: 'done', succeeded: 2, failed: 0, imported: 8, duplicates: 2 })
  })

  it('is error when all jobs failed', () => {
    const s = foldStatus([
      { id: 1, kind: 'sync', label: 'a', status: 'error', imported: 0, duplicates: 0, error: null },
    ])
    expect(s).toMatchObject({ state: 'error', failed: 1 })
  })

  it('is partial when some succeeded and some failed', () => {
    const s = foldStatus([
      { id: 1, kind: 'sync', label: 'a', status: 'success', imported: 2, duplicates: 0, error: null },
      { id: 2, kind: 'sync', label: 'b', status: 'error', imported: 0, duplicates: 0, error: null },
    ])
    expect(s).toMatchObject({ state: 'partial', succeeded: 1, failed: 1, imported: 2 })
  })
})

describe('import store', () => {
  it('loads the databases for the target picker', async () => {
    vi.mocked(api.databases.list).mockResolvedValue([
      { id: 1, owner_id: 'u1', name: 'Mine', kind: 'own', index_depth: null, global: false },
    ])
    const store = useImportStore()
    await store.loadDatabases()
    expect(store.databases).toHaveLength(1)
    expect(store.error).toBeNull()
  })

  it('surfaces a database-list load failure on error', async () => {
    vi.mocked(api.databases.list).mockRejectedValueOnce(new Error('offline'))
    const store = useImportStore()
    await store.loadDatabases()
    expect(store.error).toContain('offline')
  })

  it('records a successful sync as a job and folds it into the summary', async () => {
    vi.mocked(api.import.sync).mockResolvedValue({ imported: 12 })
    const store = useImportStore()

    await store.syncSource({ databaseId: 1, source: 'lichess', username: 'alice', token: 'tok' })
    await flushPromises()

    expect(api.import.sync).toHaveBeenCalledWith(1, 'lichess', 'alice', 'tok')
    expect(store.jobs).toHaveLength(1)
    expect(store.jobs[0]).toMatchObject({ kind: 'sync', status: 'success', imported: 12 })
    expect(store.summary).toMatchObject({ state: 'done', imported: 12 })
  })

  it('records a failed sync with its error message', async () => {
    vi.mocked(api.import.sync).mockRejectedValueOnce(new Error('no such user'))
    const store = useImportStore()

    await store.syncSource({ databaseId: 1, source: 'chesscom', username: 'ghost' })
    await flushPromises()

    expect(store.jobs[0]).toMatchObject({ status: 'error' })
    expect(store.jobs[0].error).toContain('no such user')
    expect(store.summary.state).toBe('error')
  })

  it('records a PGN upload as a job labelled by file name', async () => {
    vi.mocked(api.import.uploadPgn).mockResolvedValue({ imported: 3 })
    const store = useImportStore()

    await store.uploadPgn({ databaseId: 2, name: 'games.pgn', pgn: '[Event "x"]\n\n1. e4 *' })
    await flushPromises()

    expect(api.import.uploadPgn).toHaveBeenCalledWith(2, '[Event "x"]\n\n1. e4 *')
    expect(store.jobs[0]).toMatchObject({ kind: 'pgn', label: 'games.pgn', status: 'success', imported: 3 })
  })

  it('records duplicate drops reported by a PGN upload', async () => {
    vi.mocked(api.import.uploadPgn).mockResolvedValue({
      imported: 0,
      duplicates: 2,
      game_ids: [],
    })
    const store = useImportStore()

    await store.uploadPgn({ databaseId: 2, name: 'again.pgn', pgn: '[Event "x"]\n\n1. e4 *' })
    await flushPromises()

    expect(store.jobs[0]).toMatchObject({ status: 'success', imported: 0, duplicates: 2 })
    expect(store.summary).toMatchObject({ state: 'done', imported: 0, duplicates: 2 })
  })

  it('keeps the newest job first', async () => {
    vi.mocked(api.import.sync).mockResolvedValue({ imported: 1 })
    const store = useImportStore()
    await store.syncSource({ databaseId: 1, source: 'lichess', username: 'a' })
    await store.syncSource({ databaseId: 1, source: 'lichess', username: 'b' })
    await flushPromises()
    expect(store.jobs.map((j) => j.label)).toEqual(['lichess · b', 'lichess · a'])
  })
})
