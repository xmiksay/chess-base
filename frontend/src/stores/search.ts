// Pinia store backing the search surfaces (issue #69): header/metadata search and
// the position / opening-tree explorer. Pure query-state and tree-navigation
// logic lives in lib/headerQuery + lib/openingTree; this store just holds
// reactive state and calls the API. Both flows surface their own loading/error.

import { defineStore } from 'pinia'
import { computed, reactive, ref } from 'vue'
import { api } from '../api'
import { emptyQuery, isEmptyQuery, toParams } from '../lib/headerQuery'
import { emptyFilter, toParams as toFilterParams } from '../lib/positionFilter'
import { lineFen, moveToSan, replayLine } from '../lib/openingTree'
import type { BoardMove, GameRow, HeaderQuery, MoveStat, PositionFilter } from '../types'

export const useSearchStore = defineStore('search', () => {
  // --- Header search --------------------------------------------------------
  // Backed by the keyset-paginated `/api/search/headers` endpoint: a run fetches
  // the first page and remembers `nextCursor`; `loadMore` appends the next page.
  const query = reactive<HeaderQuery>(emptyQuery())
  const results = ref<GameRow[]>([])
  const nextCursor = ref<string | null>(null)
  const searched = ref(false) // has a header search been run at least once?
  const headerLoading = ref(false)
  const headerError = ref<string | null>(null)

  const queryIsEmpty = computed(() => isEmptyQuery(query))
  const hasMore = computed(() => nextCursor.value != null)

  async function fetchPage(cursor: string | null) {
    const params = toParams(query)
    if (cursor) params.cursor = cursor
    return api.search.headers(params)
  }

  async function runHeaderSearch() {
    headerLoading.value = true
    headerError.value = null
    try {
      const page = await fetchPage(null)
      results.value = page.games
      nextCursor.value = page.next_cursor ?? null
      searched.value = true
    } catch (e) {
      headerError.value = String((e as Error)?.message ?? e)
      results.value = []
      nextCursor.value = null
    } finally {
      headerLoading.value = false
    }
  }

  async function loadMore() {
    if (!nextCursor.value || headerLoading.value) return
    headerLoading.value = true
    headerError.value = null
    try {
      const page = await fetchPage(nextCursor.value)
      results.value = [...results.value, ...page.games]
      nextCursor.value = page.next_cursor ?? null
    } catch (e) {
      headerError.value = String((e as Error)?.message ?? e)
    } finally {
      headerLoading.value = false
    }
  }

  function resetQuery() {
    Object.assign(query, emptyQuery())
    results.value = []
    nextCursor.value = null
    searched.value = false
    headerError.value = null
  }

  /** Drop games from the results after a bulk delete (issue #171). */
  function removeResults(ids: Set<number>) {
    results.value = results.value.filter((g) => !ids.has(g.id))
  }

  // --- Position / opening-tree explorer -------------------------------------
  const line = ref<string[]>([]) // SAN moves from the start position
  const tree = ref<MoveStat[]>([]) // MoveStat rows for the current position
  const games = ref<GameRow[]>([]) // GameHit rows reaching the current position
  // The MoveStat row for the move that reached the current line's final
  // position, captured at the moment it was played (issue #173: the explorer's
  // "N games, W/D/L" for that position, offered as an "Add line to study"
  // comment). `null` at the start position or after navigating away from the
  // move it was captured for (back/reset).
  const lastMoveStat = ref<MoveStat | null>(null)
  const explorerLoading = ref(false)
  const explorerError = ref<string | null>(null)
  // Player/color/date filter narrowing which games' continuations count
  // (issue #172); blank ⇒ unfiltered, mirroring the header-search convention.
  const filter = reactive<PositionFilter>(emptyFilter())

  // Board state derived purely from the line (no separate chess instance to
  // keep in sync). `position` carries fen/dests/lastMove/turnColor.
  const position = computed(() => replayLine(line.value))
  const fen = computed(() => position.value.fen)

  async function loadPosition() {
    explorerLoading.value = true
    explorerError.value = null
    const target = lineFen(line.value)
    const params = toFilterParams(filter)
    try {
      const [t, g] = await Promise.all([
        api.search.tree(target, params),
        api.search.games(target, 50, params),
      ])
      // Guard against an out-of-order response after a rapid click.
      if (target === lineFen(line.value)) {
        tree.value = t
        games.value = g
      }
    } catch (e) {
      explorerError.value = String((e as Error)?.message ?? e)
      tree.value = []
      games.value = []
    } finally {
      explorerLoading.value = false
    }
  }

  /** Clear the explorer filter and reload the current position. */
  function resetFilter() {
    Object.assign(filter, emptyFilter())
    return loadPosition()
  }

  /** Descend the tree by one SAN continuation (from a tree row). */
  function playSan(san: string) {
    lastMoveStat.value = tree.value.find((m) => m.san === san) ?? null
    line.value = [...line.value, san]
    return loadPosition()
  }

  /** Play a board drag `{from,to}`; ignored when the move is illegal. */
  function playMove({ from, to, promotion }: BoardMove): string | null {
    const san = moveToSan(line.value, from, to, promotion)
    if (!san) return null
    playSan(san)
    return san
  }

  /** Step back one move; no-op at the root. */
  function back() {
    if (line.value.length === 0) return
    line.value = line.value.slice(0, -1)
    lastMoveStat.value = null
    return loadPosition()
  }

  /** Return to the start position. */
  function resetBoard() {
    line.value = []
    lastMoveStat.value = null
    return loadPosition()
  }

  return {
    // header search
    query,
    results,
    nextCursor,
    hasMore,
    searched,
    headerLoading,
    headerError,
    queryIsEmpty,
    runHeaderSearch,
    loadMore,
    resetQuery,
    removeResults,
    // explorer
    line,
    tree,
    games,
    lastMoveStat,
    explorerLoading,
    explorerError,
    filter,
    resetFilter,
    position,
    fen,
    loadPosition,
    playSan,
    playMove,
    back,
    resetBoard,
  }
})
