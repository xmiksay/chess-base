// Pinia store for the study editor's lifecycle: list the visible studies, open
// one (loading its move tree), create / import-from-PGN / rename / delete, and
// export back to PGN. Thin wrapper over `api.studies` (issue #9) — the per-move
// tree edits live in the engine/board flow and the node-mutation endpoints.

import { defineStore } from 'pinia'
import { ref } from 'vue'
import { api } from '../api.js'

export const useStudiesStore = defineStore('studies', () => {
  const list = ref([]) // summaries: { id, database_id, name, global, owner_id }
  const current = ref(null) // open study with its full move tree
  const loading = ref(false)
  const error = ref(null)

  /** Run `fn`, surfacing failures on `error` and toggling `loading`. */
  async function _run(fn) {
    loading.value = true
    error.value = null
    try {
      return await fn()
    } catch (e) {
      error.value = String(e.message ?? e)
      throw e
    } finally {
      loading.value = false
    }
  }

  /** Refresh the list of studies visible to the caller. */
  async function refresh() {
    list.value = await _run(() => api.studies.list())
    return list.value
  }

  /** Load a study (with its move tree) into `current`. */
  async function open(id) {
    current.value = await _run(() => api.studies.get(id))
    return current.value
  }

  /** Create an empty study, open it, and refresh the list. */
  async function create(databaseId, name, global = false) {
    const study = await _run(() => api.studies.create(databaseId, name, global))
    current.value = study
    await refresh()
    return study
  }

  /** Import a PGN as a new study, open it, and refresh the list. */
  async function importPgn(databaseId, name, pgn, global = false) {
    const study = await _run(() => api.studies.importPgn(databaseId, name, pgn, global))
    current.value = study
    await refresh()
    return study
  }

  /** Export a study to PGN movetext. */
  function exportPgn(id) {
    return _run(() => api.studies.exportPgn(id))
  }

  /** Rename a study; keeps `current` and the list in sync. */
  async function rename(id, name) {
    const study = await _run(() => api.studies.rename(id, name))
    if (current.value?.id === id) current.value = study
    const summary = list.value.find((s) => s.id === id)
    if (summary) summary.name = name
    return study
  }

  /** Delete a study; clears `current` if it was the open one. */
  async function remove(id) {
    await _run(() => api.studies.remove(id))
    if (current.value?.id === id) current.value = null
    list.value = list.value.filter((s) => s.id !== id)
  }

  return {
    list,
    current,
    loading,
    error,
    refresh,
    open,
    create,
    importPgn,
    exportPgn,
    rename,
    remove,
  }
})
