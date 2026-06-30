// Pinia store for the game browser (issue #68): an offset-paginated, sortable
// game list for a selected database plus the currently opened game on the shared
// variation-tree board (issue #136). The board state machine lives in the
// `useTreeBoard` composable (issue #134) so the Game-review board shares one
// implementation with Analyse and Studies; `open(id)` seeds it from the game's
// parsed tree (`GET /api/games/{id}/tree`, #135) so PGN variations are kept and
// off-line moves branch rather than truncate.

import { defineStore } from 'pinia'
import { ref, computed } from 'vue'
import { api } from '../api'
import { useTreeBoard } from '../lib/useTreeBoard'
import { mainlinePath as mainlinePathOf } from '../lib/moveTree'
import { graftReviewVariations } from '../lib/reviewTree'
import { STARTPOS_FEN } from '../lib/fen'
import type { GameDetail, GameReview, GameRow, StudySummary } from '../types'

/** Sort fields the list supports (mirrors the backend `GameSort`). */
export type GameSortField = 'date' | 'result' | 'eco'

export const useGamesStore = defineStore('games', () => {
  const board = useTreeBoard()

  const databaseId = ref<number | null>(null)
  const games = ref<GameRow[]>([]) // the current page's rows
  const total = ref(0) // total games in the database (for the paginator)
  const page = ref(0) // 0-based index of the current page
  const limit = ref(50) // page size (echoed back by the server after clamping)
  const sort = ref<GameSortField>('date') // default: newest first
  const dir = ref<'asc' | 'desc'>('desc')
  const loading = ref(false)
  const error = ref<string | null>(null)

  // Total pages (≥ 1 so the "Page x of y" label is always sensible).
  const pageCount = computed(() => Math.max(1, Math.ceil(total.value / limit.value)))
  const hasPrev = computed(() => page.value > 0)
  const hasNext = computed(() => page.value + 1 < pageCount.value)
  // 1-based row range shown, e.g. "showing 51–100 of 240" (0–0 when empty).
  const rangeStart = computed(() => (total.value === 0 ? 0 : page.value * limit.value + 1))
  const rangeEnd = computed(() => Math.min(total.value, (page.value + 1) * limit.value))

  // The opened game's headers; the board composable holds its move tree + cursor.
  const openGame = ref<GameDetail | null>(null)
  // Studies saved as analyses of the open game (issue #164).
  const linkedStudies = ref<StudySummary[]>([])

  /** Select a database and load its first page, replacing any prior list. */
  async function selectDatabase(id: number) {
    databaseId.value = id
    page.value = 0
    await fetchPage()
  }

  /** Fetch the current page (page/sort/dir) for the selected database. */
  async function fetchPage() {
    if (databaseId.value == null || loading.value) return
    loading.value = true
    error.value = null
    try {
      const res = await api.games.list(databaseId.value, {
        page: page.value,
        limit: limit.value,
        sort: sort.value,
        dir: dir.value,
      })
      games.value = res.games
      total.value = res.total
      page.value = res.page
      limit.value = res.limit
    } catch (e) {
      error.value = (e as Error)?.message ?? String(e)
    } finally {
      loading.value = false
    }
  }

  /** Jump to a page (clamped to `[0, pageCount-1]`); no-op if unchanged. */
  async function goToPage(target: number) {
    const clamped = Math.min(Math.max(0, target), pageCount.value - 1)
    if (clamped === page.value) return
    page.value = clamped
    await fetchPage()
  }

  /**
   * Sort by `field`: clicking the active field flips direction, a new field
   * starts at its natural default (date newest-first, others ascending). Resets
   * to the first page so the user sees the new order's head.
   */
  async function setSort(field: GameSortField) {
    if (sort.value === field) {
      dir.value = dir.value === 'asc' ? 'desc' : 'asc'
    } else {
      sort.value = field
      dir.value = field === 'date' ? 'desc' : 'asc'
    }
    page.value = 0
    await fetchPage()
  }

  /**
   * Fetch a game by id (headers + parsed variation tree) and seed the board at
   * the start position. The tree keeps `(…)` sub-variations the flat viewer
   * dropped (#135), and the board lets the user branch their own lines (#134).
   */
  async function open(id: number) {
    loading.value = true
    error.value = null
    try {
      const [game, tree] = await Promise.all([api.games.get(id), api.games.tree(id)])
      openGame.value = game
      board.load(tree, STARTPOS_FEN)
      await loadLinkedStudies(id)
    } catch (e) {
      error.value = (e as Error)?.message ?? String(e)
    } finally {
      loading.value = false
    }
  }

  /** Refresh the analyses (studies) linked to a game (issue #164). */
  async function loadLinkedStudies(id: number) {
    try {
      linkedStudies.value = await api.games.linkedStudies(id)
    } catch (e) {
      error.value = (e as Error)?.message ?? String(e)
    }
  }

  /**
   * Save the open game as a study (issue #164), optionally filed under a folder
   * and engine-analysed. Refreshes the linked-analyses list on success so the new
   * study shows immediately. Throws on failure so the caller can surface the
   * message (e.g. the 503 when `analyse` is requested without an engine).
   */
  async function saveAsStudy(body: {
    name: string
    folder_id?: number | null
    analyse?: boolean
    depth?: number
  }): Promise<StudySummary | null> {
    if (!openGame.value) return null
    const study = await api.games.saveAsStudy(openGame.value.id, body)
    await loadLinkedStudies(openGame.value.id)
    return study
  }

  /** Node ids along the mainline; the array index is the ply (0 = start). */
  function mainlinePath(): number[] {
    return mainlinePathOf(board.tree.value)
  }

  /** Mainline ply of `nodeId` (0 = root), or null for an off-mainline node. */
  function plyOf(nodeId: number): number | null {
    const i = mainlinePath().indexOf(nodeId)
    return i < 0 ? null : i
  }

  /** The mainline node at `ply` (the inverse of `plyOf`), or null past the end. */
  function nodeAtPly(ply: number): number | null {
    return mainlinePath()[ply] ?? null
  }

  /**
   * Graft the engine review's critical lines onto the live board tree (#136):
   * each inaccuracy/mistake/blunder sprouts the engine's better line as a
   * sibling variation. Idempotent, and the mainline is never reordered so the
   * ply mapping above stays stable and the user's own branches survive.
   */
  function graftReview(review: GameReview) {
    board.tree.value = graftReviewVariations(board.tree.value, review)
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
    ...board,
    databaseId,
    games,
    total,
    page,
    limit,
    sort,
    dir,
    pageCount,
    hasPrev,
    hasNext,
    rangeStart,
    rangeEnd,
    loading,
    error,
    openGame,
    linkedStudies,
    loadLinkedStudies,
    saveAsStudy,
    selectDatabase,
    fetchPage,
    goToPage,
    setSort,
    open,
    mainlinePath,
    plyOf,
    nodeAtPly,
    graftReview,
    exportPgn,
  }
})
