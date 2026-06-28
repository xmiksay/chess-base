// Pinia store for the game browser (issue #68): a keyset-paginated game list for
// a selected database plus the currently opened game's replayable positions.

import { defineStore } from 'pinia'
import { ref, computed } from 'vue'
import { api } from '../api.js'
import { positionsFromPgn, navigate } from '../lib/pgnViewer.js'

export const useGamesStore = defineStore('games', () => {
  const databaseId = ref(null)
  const games = ref([]) // accumulated GameSummary rows across pages
  const cursor = ref(null) // keyset cursor for the next page; null ⇒ none loaded yet
  const hasMore = ref(false)
  const loading = ref(false)
  const error = ref(null)

  // Opened game + its replay state.
  const openGame = ref(null) // GameDetail
  const positions = ref([{ ply: 0, san: null, fen: undefined, lastMove: null }])
  const ply = ref(0)

  const current = computed(() => positions.value[ply.value] ?? positions.value[0])
  const fen = computed(() => current.value?.fen)
  const lastMove = computed(() => current.value?.lastMove ?? null)
  const atStart = computed(() => ply.value <= 0)
  const atEnd = computed(() => ply.value >= positions.value.length - 1)

  /** Select a database and load its first page, replacing any prior list. */
  async function selectDatabase(id) {
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
      error.value = e.message ?? String(e)
    } finally {
      loading.value = false
    }
  }

  /** Fetch a game by id and load it into the board viewer at the start position. */
  async function open(id) {
    loading.value = true
    error.value = null
    try {
      const game = await api.games.get(id)
      openGame.value = game
      positions.value = positionsFromPgn(game.pgn)
      ply.value = 0
    } catch (e) {
      error.value = e.message ?? String(e)
    } finally {
      loading.value = false
    }
  }

  /** Step the viewer: 'first' | 'prev' | 'next' | 'last', or a ply number. */
  function go(action) {
    ply.value = navigate(ply.value, action, positions.value.length)
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
  }
})
