// Pinia store for the game-import view (issue #70): list the databases the
// caller may import into, run Lichess/Chess.com syncs and PGN uploads, and track
// each as a job whose status the UI folds into an overall summary.
//
// `foldStatus` is pure and exported so the status-folding rule is unit-tested
// directly (and reused as the store's `summary` computed).

import { defineStore } from 'pinia'
import { computed, reactive, ref } from 'vue'
import { api } from '../api.js'

// Monotonic id for jobs so list rendering has a stable `:key`.
let nextJobId = 1

/**
 * Fold a list of import jobs into an overall summary. A job is
 * `{ status: 'running' | 'success' | 'error', imported: number }`.
 *
 * `state` is the headline:
 *  - `idle`    — no jobs yet
 *  - `running` — at least one job still in flight
 *  - `error`   — all finished jobs failed
 *  - `partial` — some succeeded, some failed
 *  - `done`    — all finished jobs succeeded
 */
export function foldStatus(jobs) {
  const running = jobs.filter((j) => j.status === 'running').length
  const succeeded = jobs.filter((j) => j.status === 'success').length
  const failed = jobs.filter((j) => j.status === 'error').length
  const imported = jobs.reduce((sum, j) => sum + (j.imported || 0), 0)

  let state
  if (jobs.length === 0) state = 'idle'
  else if (running > 0) state = 'running'
  else if (failed === 0) state = 'done'
  else if (succeeded === 0) state = 'error'
  else state = 'partial'

  return { total: jobs.length, running, succeeded, failed, imported, state }
}

export const useImportStore = defineStore('import', () => {
  const databases = ref([]) // { id, name, kind, global, ... }
  const jobs = ref([]) // newest first
  const error = ref(null) // database-list load failure

  const summary = computed(() => foldStatus(jobs.value))

  /** Load the databases the caller can see (own ∪ global) for the target picker. */
  async function loadDatabases() {
    error.value = null
    try {
      databases.value = await api.databases.list()
    } catch (e) {
      error.value = String(e.message ?? e)
    }
    return databases.value
  }

  /** Run an import (`fn` resolves to `{ imported }`), tracking it as a job. */
  async function _track(kind, label, fn) {
    const job = reactive({ id: nextJobId++, kind, label, status: 'running', imported: 0, error: null })
    jobs.value.unshift(job)
    try {
      const res = await fn()
      job.status = 'success'
      job.imported = res?.imported ?? 0
    } catch (e) {
      job.status = 'error'
      job.error = String(e.message ?? e)
    }
    return job
  }

  /** Trigger a provider sync into `databaseId`. */
  function syncSource({ databaseId, source, username, token }) {
    return _track('sync', `${source} · ${username}`, () =>
      api.import.sync(databaseId, source, username, token),
    )
  }

  /** Upload a PGN (`pgn` text, `name` for the job label) into `databaseId`. */
  function uploadPgn({ databaseId, name, pgn }) {
    return _track('pgn', name || 'PGN upload', () => api.import.uploadPgn(databaseId, pgn))
  }

  return { databases, jobs, error, summary, loadDatabases, syncSource, uploadPgn }
})
