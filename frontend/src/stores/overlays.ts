// Pinia store for the position-derived board overlays (issue #123): the Threats
// layer (`/api/threats`) and the Database master-moves layer (`/api/search/tree`).
// It holds the computed shapes for the current position; the analysis view drives
// (re)loading on position / toggle changes and composes them with the engine
// Plans layer via lib/boardShapes. The engine Plans layer stays in stores/engine.

import { defineStore } from 'pinia'
import { ref, shallowRef } from 'vue'
import type { DrawShape } from 'chessground/draw'
import { api } from '../api'
import { shapesToDrawShapes } from '../lib/boardShapes'
import { masterMovesToShapes } from '../lib/masterShapes'

export const useOverlaysStore = defineStore('overlays', () => {
  const threats = shallowRef<DrawShape[]>([])
  const master = shallowRef<DrawShape[]>([])
  const error = ref<string | null>(null)

  /** Fetch the threatened-piece arrows for `fen`; clears on failure. */
  async function loadThreats(fen: string) {
    try {
      threats.value = shapesToDrawShapes(await api.search.threats(fen))
      error.value = null
    } catch (e) {
      threats.value = []
      error.value = String((e as Error)?.message ?? e)
    }
  }

  /** Fetch + map the most-played master continuations for `fen`; clears on failure. */
  async function loadMaster(fen: string) {
    try {
      master.value = masterMovesToShapes(fen, await api.search.tree(fen))
      error.value = null
    } catch (e) {
      master.value = []
      error.value = String((e as Error)?.message ?? e)
    }
  }

  function clearThreats() {
    threats.value = []
  }

  function clearMaster() {
    master.value = []
  }

  return { threats, master, error, loadThreats, loadMaster, clearThreats, clearMaster }
})
