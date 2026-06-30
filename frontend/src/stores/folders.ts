// Pinia store for the study folder hierarchy (issue #164): list the visible
// folders, create / rename / move / delete them, and look up a folder's children
// for the recursive tree render. Thin wrapper over `api.folders`; the studies
// themselves stay in the `studies` store (each carries its `folder_id`).

import { defineStore } from 'pinia'
import { ref } from 'vue'
import { api } from '../api'
import type { FolderSummary } from '../types'

export const useFoldersStore = defineStore('folders', () => {
  const list = ref<FolderSummary[]>([])
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

  /** Refresh the list of folders visible to the caller. */
  async function refresh() {
    list.value = await _run(() => api.folders.list())
    return list.value
  }

  /** Create a folder under `parentId` (null ⇒ root), then refresh. */
  async function create(name: string, parentId: number | null = null) {
    const folder = await _run(() => api.folders.create(name, parentId))
    await refresh()
    return folder
  }

  /** Rename a folder; updates the row in `list` in place. */
  async function rename(id: number, name: string) {
    const folder = await _run(() => api.folders.rename(id, name))
    const row = list.value.find((f) => f.id === id)
    if (row) row.name = folder.name
    return folder
  }

  /** Move a folder under `parentId` (null ⇒ root), then refresh. */
  async function move(id: number, parentId: number | null) {
    const folder = await _run(() => api.folders.move(id, parentId))
    await refresh()
    return folder
  }

  /** Delete a folder (cascades to children, unfiles its studies), then refresh. */
  async function remove(id: number) {
    await _run(() => api.folders.remove(id))
    await refresh()
  }

  /** The folders whose parent is `parentId` (null ⇒ the root folders). */
  function childrenOf(parentId: number | null): FolderSummary[] {
    return list.value.filter((f) => f.parent_id === parentId)
  }

  return { list, loading, error, refresh, create, rename, move, remove, childrenOf }
})
