// Pinia store for the collection (databases) browser (issue #67): list the
// caller's databases plus global (admin-managed) ones, and create / rename /
// delete the ones the caller may write. Thin wrapper over `api.databases`.
//
// The list mixes the caller's own databases with global (`global: true`,
// owner_id IS NULL) ones. A non-admin sees global databases read-only — writes
// are admin-gated server-side, so `canWrite` mirrors that guard in the UI.

import { defineStore } from 'pinia'
import { ref } from 'vue'
import { api } from '../api.js'

export const useCollectionsStore = defineStore('collections', () => {
  const list = ref([]) // { id, owner_id, name, kind, index_depth, global }
  const isAdmin = ref(false)
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

  /** Whether the caller may rename/delete `db`: own databases always, global
   *  ones only as admin (mirrors the server's write guard). */
  function canWrite(db) {
    return !db.global || isAdmin.value
  }

  /** Refresh the visible databases and the caller's admin flag. */
  async function refresh() {
    await _run(async () => {
      isAdmin.value = (await api.whoami()).is_admin === true
      list.value = await api.databases.list()
    })
    return list.value
  }

  /** Create a database and add it to the list. `global` requires admin. */
  async function create(name, kind, global = false) {
    const db = await _run(() => api.databases.create(name, kind, global))
    list.value.push(db)
    return db
  }

  /** Rename a database; keeps the list summary in sync. */
  async function rename(id, name) {
    const db = await _run(() => api.databases.rename(id, name))
    const summary = list.value.find((d) => d.id === id)
    if (summary) summary.name = db.name
    return db
  }

  /** Delete a database and drop it from the list. */
  async function remove(id) {
    await _run(() => api.databases.remove(id))
    list.value = list.value.filter((d) => d.id !== id)
  }

  return {
    list,
    isAdmin,
    loading,
    error,
    canWrite,
    refresh,
    create,
    rename,
    remove,
  }
})
