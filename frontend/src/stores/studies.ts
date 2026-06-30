// Pinia store for the study editor's lifecycle: list the visible studies, open
// one (loading its move tree), create / import-from-PGN / rename / delete, and
// export back to PGN. Thin wrapper over `api.studies` (issue #9). This store owns
// the open study in `current`; the per-move tree edits + board interaction live
// in the `studyEditor` store (issue #8), which reads/writes `current`.

import { defineStore } from 'pinia'
import { ref } from 'vue'
import { api } from '../api'
import type { DangerMapBody, GenerateBody, Study, StudySummary } from '../types'

export const useStudiesStore = defineStore('studies', () => {
  const list = ref<StudySummary[]>([])
  const current = ref<Study | null>(null) // open study with its full move tree
  const loading = ref(false)
  const error = ref<string | null>(null)

  /** Run `fn`, surfacing failures on `error` and toggling `loading`. */
  async function _run<T>(fn: () => Promise<T>): Promise<T> {
    loading.value = true
    error.value = null
    try {
      return await fn()
    } catch (e) {
      error.value = String((e as Error)?.message ?? e)
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
  async function open(id: number) {
    current.value = await _run(() => api.studies.get(id))
    return current.value
  }

  /** Create an empty study, open it, and refresh the list. */
  async function create(databaseId: number, name: string, global = false) {
    const study = await _run(() => api.studies.create(databaseId, name, global))
    current.value = study
    await refresh()
    return study
  }

  /** Import a PGN as a new study, open it, and refresh the list. */
  async function importPgn(databaseId: number, name: string, pgn: string, global = false) {
    const study = await _run(() => api.studies.importPgn(databaseId, name, pgn, global))
    current.value = study
    await refresh()
    return study
  }

  /** Generate an LLM-assisted study (issue #119); refresh the list on success. */
  async function generate(body: GenerateBody) {
    const view = await _run(() => api.studies.generate(body))
    await refresh()
    return view
  }

  /** Generate a danger-map study (issue #131); refresh the list on success. */
  async function generateDangerMap(body: DangerMapBody) {
    const view = await _run(() => api.studies.generateDangerMap(body))
    await refresh()
    return view
  }

  /**
   * Fetch a study's PGN for download (issue #120). `withEval` (default true)
   * keeps the per-move `[%eval]` annotations; `false` exports plain movetext.
   * Returns the text; the view triggers the file download.
   */
  function exportPgn(id: number, withEval = true) {
    return _run(() => api.studies.exportPgn(id, { eval: withEval }))
  }

  /** Rename a study; keeps `current` and the list in sync. */
  async function rename(id: number, name: string) {
    const study = await _run(() => api.studies.rename(id, name))
    if (current.value?.id === id) current.value = study
    const summary = list.value.find((s) => s.id === id)
    if (summary) summary.name = name
    return study
  }

  /** Delete a study; clears `current` if it was the open one. */
  async function remove(id: number) {
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
    generate,
    generateDangerMap,
    importPgn,
    exportPgn,
    rename,
    remove,
  }
})
