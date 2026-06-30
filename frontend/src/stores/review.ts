// Pinia store for the fast engine-only full-game review (issue #119, Mode A).
// Holds the most recent `GameReview` for one game plus a per-ply index for O(1)
// lookup from the move list. Thin wrapper over `api.games.analyse`.

import { defineStore } from 'pinia'
import { ref, computed } from 'vue'
import { api } from '../api'
import { useGamesStore } from './games'
import type { GameReview, MoveReview } from '../types'

export const useReviewStore = defineStore('review', () => {
  const review = ref<GameReview | null>(null)
  const gameId = ref<number | null>(null)
  const loading = ref(false)
  const error = ref<string | null>(null)

  /** Reviewed moves indexed by their 1-based ply, for fast move-list lookup. */
  const byPly = computed(() => {
    const map = new Map<number, MoveReview>()
    for (const m of review.value?.moves ?? []) map.set(m.ply, m)
    return map
  })

  /**
   * The reviewed move at the board's current node (issue #136). The board cursor
   * is a tree node, so we map it back to its mainline ply via the games store;
   * off-mainline nodes (PGN/engine/user variations) have no review and yield
   * null. The `byPly` index stays mainline-ply keyed.
   */
  const currentMove = computed<MoveReview | null>(() => {
    const games = useGamesStore()
    const ply = games.plyOf(games.currentId)
    return ply == null ? null : (byPly.value.get(ply) ?? null)
  })

  /** Run the engine review for a game; surfaces failures on `error`. */
  async function analyse(id: number, depth?: number) {
    loading.value = true
    error.value = null
    try {
      const result = await api.games.analyse(id, depth)
      review.value = result
      gameId.value = id
      return result
    } catch (e) {
      error.value = String((e as Error)?.message ?? e)
      review.value = null
      gameId.value = null
    } finally {
      loading.value = false
    }
  }

  /** Reset the store (call when a different game is opened). */
  function clear() {
    review.value = null
    gameId.value = null
    error.value = null
  }

  return { review, gameId, loading, error, byPly, currentMove, analyse, clear }
})
