// Pinia store for the game browser (issue #68): a keyset-paginated game list for
// a selected database plus the currently opened game's replayable positions.

import { defineStore } from 'pinia'
import { ref, computed } from 'vue'
import { api } from '../api'
import { positionsFromPgn, navigate } from '../lib/pgnViewer'
import type { GameDetail, GameRow, ViewerPosition } from '../types'

export const useGamesStore = defineStore('games', () => {
  const databaseId = ref<number | null>(null)
  const games = ref<GameRow[]>([]) // accumulated GameSummary rows across pages
  const cursor = ref<number | null>(null) // keyset cursor for the next page; null ⇒ none loaded yet
  const hasMore = ref(false)
  const loading = ref(false)
  const error = ref<string | null>(null)

  // Opened game + its replay state.
  const openGame = ref<GameDetail | null>(null)
  const positions = ref<ViewerPosition[]>([{ ply: 0, san: null, fen: undefined, lastMove: null }])
  const ply = ref(0)

  const current = computed(() => positions.value[ply.value] ?? positions.value[0])
  const fen = computed(() => current.value?.fen)
  const lastMove = computed(() => current.value?.lastMove ?? null)
  const atStart = computed(() => ply.value <= 0)
  const atEnd = computed(() => ply.value >= positions.value.length - 1)

  /** Select a database and load its first page, replacing any prior list. */
  async function selectDatabase(id: number) {
    databaseId.value = id
    games.value = []
    cursor.value = null
    hasMore.value = false
    await loadMore()
  }

  /** Load the next keyset page for the selected database. */
  async function loadMore() {
    if (databaseId.value == null || loading.value) return
    loading.value = true
    error.value = null
    try {
      const page = await api.games.list(databaseId.value, {
        after: cursor.value ?? undefined,
      })
      games.value.push(...page.games)
      cursor.value = page.next_cursor
      hasMore.value = page.next_cursor != null
    } catch (e) {
      error.value = (e as Error)?.message ?? String(e)
    } finally {
      loading.value = false
    }
  }

  /** Fetch a game by id and load it into the board viewer at the start position. */
  async function open(id: number) {
    loading.value = true
    error.value = null
    try {
      const game = await api.games.get(id)
      openGame.value = game
      positions.value = positionsFromPgn(game.pgn)
      ply.value = 0
    } catch (e) {
      error.value = (e as Error)?.message ?? String(e)
    } finally {
      loading.value = false
    }
  }

  /** Step the viewer: 'first' | 'prev' | 'next' | 'last', or a ply number. */
  function go(action: string | number) {
    ply.value = navigate(ply.value, action, positions.value.length)
  }

  /**
   * Fetch the open game's PGN for download (issue #120): verbatim, or — with
   * `annotated` — with the engine analysis (`[%eval]` + NAGs + why-notes)
   * embedded. Returns the PGN text (the view triggers the file download), or
   * `null` if no game is open or the request failed (surfaced on `error`).
   */
  async function exportPgn(annotated = false): Promise<string | null> {
    if (!openGame.value) return null
    error.value = null
    try {
      return await api.games.exportPgn(openGame.value.id, { annotated })
    } catch (e) {
      error.value = (e as Error)?.message ?? String(e)
      return null
    }
  }

  return {
    databaseId,
    games,
    hasMore,
    loading,
    error,
    openGame,
    positions,
    ply,
    current,
    fen,
    lastMove,
    atStart,
    atEnd,
    selectDatabase,
    loadMore,
    open,
    go,
    exportPgn,
  }
})
