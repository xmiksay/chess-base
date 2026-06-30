// Pinia store for the engine-only danger overlay (issue #156): holds the walked
// DangerTree for a spine (`POST /api/studies/danger-map`) plus its flat panel
// digest. The study view derives the per-position arrows from `tree` via
// lib/dangerShapes and lists `roles` in a side panel. No LLM on this path, so it
// works on a local / no-key install — unlike the GenerateDangerMapDialog flow.

import { defineStore } from 'pinia'
import { ref, shallowRef } from 'vue'
import { api } from '../api'
import { dangerRoles, type DangerRoleRow } from '../lib/dangerShapes'
import type { DangerTree, DangerWalkBody } from '../types'

export const useDangerStore = defineStore('danger', () => {
  const tree = shallowRef<DangerTree | null>(null)
  const roles = ref<DangerRoleRow[]>([])
  const loading = ref(false)
  const error = ref<string | null>(null)

  /** Walk `body.spine_pgn` for danger; populate the tree + roles, or clear on failure. */
  async function load(body: DangerWalkBody) {
    loading.value = true
    error.value = null
    try {
      const result = await api.studies.dangerMap(body)
      tree.value = result.tree
      roles.value = dangerRoles(result.tree)
    } catch (e) {
      tree.value = null
      roles.value = []
      error.value = String((e as Error)?.message ?? e)
    } finally {
      loading.value = false
    }
  }

  function clear() {
    tree.value = null
    roles.value = []
    error.value = null
  }

  return { tree, roles, loading, error, load, clear }
})
