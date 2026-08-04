<script setup lang="ts">
// Game browser (issue #68): pick a database, page through its games, open one on
// the shared variation-tree board (issue #136) and explore it — step the cursor,
// click moves/variations in the tree, or play an off-line move to branch. The
// engine review (analyze/export, eval graph, why-note) lives in GameReviewPanel.
import { onMounted, onUnmounted, ref, watch } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import Board from '../components/Board.vue'
import BoardControls from '../components/BoardControls.vue'
import MoveTree from '../components/MoveTree.vue'
import MoveComment from '../components/MoveComment.vue'
import EnginePanel from '../components/EnginePanel.vue'
import GameReviewPanel from '../components/GameReviewPanel.vue'
import ShareToggle from '../components/ShareToggle.vue'
import { api } from '../api'
import { useGamesStore, type GameSortField } from '../stores/games'
import { useReviewStore } from '../stores/review'
import { useSettingsStore } from '../stores/settings'
import { useAuthStore } from '../stores/auth'
import { useBoardOverlays } from '../lib/useBoardOverlays'
import { numericParam } from '../lib/routeParam'
import { downloadText } from '../lib/download'
import type { BoardMove, Database, GameRow } from '../types'

const games = useGamesStore()
const review = useReviewStore()
const settings = useSettingsStore()
const auth = useAuthStore()
const router = useRouter()
const route = useRoute()

// Multi-select for merging games into one repertoire study (issue #170). Ids stay
// valid across the current database's pages, so a selection survives paging.
const selected = ref<Set<number>>(new Set())
const merging = ref(false)
const mergeError = ref<string | null>(null)

/** Toggle a game's membership in the merge selection. */
function toggleSelected(id: number) {
  const next = new Set(selected.value)
  if (!next.delete(id)) next.add(id)
  selected.value = next
}

/** Merge the selected games into a new repertoire study, then open the editor. */
async function mergeSelected() {
  const gameIds = [...selected.value]
  if (gameIds.length < 2) return
  const name = window.prompt(`Name for the repertoire study (${gameIds.length} games):`)
  if (name == null || !name.trim()) return
  merging.value = true
  mergeError.value = null
  try {
    await api.studies.mergeGames({ game_ids: gameIds, name: name.trim() })
    selected.value = new Set()
    await router.push({ name: 'studies' })
  } catch (e) {
    mergeError.value = String((e as Error)?.message ?? e)
  } finally {
    merging.value = false
  }
}

const databases = ref<Database[]>([])
const selectedDb = ref<number | null>(null)
const loadError = ref<string | null>(null)
// Engine capability flag from `/api/health`; null until fetched.
const engineEnabled = ref<boolean | null>(null)

// Composed overlay layers (plans/threats/master) driven by the board's live FEN.
// `clearArrows` (issue #190) turns off every layer so the live engine-analysis
// arrows disappear and stay off until the user re-enables a toggle.
const { boardShapes, clear: clearArrows } = useBoardOverlays(() => games.fen)

/** A "White – Black" label for a game row, tolerating missing names. */
function players(g: GameRow): string {
  return `${g.white ?? '?'} – ${g.black ?? '?'}`
}

/** The sort-direction arrow for a column header, blank when it isn't active. */
function sortArrow(field: GameSortField): string {
  if (games.sort !== field) return ''
  return games.dir === 'asc' ? ' ▲' : ' ▼'
}

// Playing an off-line move branches a variation (useTreeBoard follow-or-branch).
function onMove({ from, to }: BoardMove) {
  games.playMove({ from, to })
}

/** Delete a game from the open database, after confirming. */
async function onDelete(g: GameRow) {
  if (!window.confirm(`Delete "${players(g)}"? This cannot be undone.`)) return
  try {
    await games.remove(g.id)
  } catch (e) {
    loadError.value = String((e as Error)?.message ?? e)
  }
}

// Sharing (issue #213, ADR-0045): the toggle in the header flips `public` on
// the open game; the link is the ordinary `/games/:id` deep link, which the
// backend now serves anonymously once flagged.
const publicToggling = ref(false)

function shareUrl(id: number): string {
  return `${window.location.origin}${router.resolve({ name: 'games', params: { id: String(id) } }).href}`
}

async function onTogglePublic(next: boolean) {
  if (!games.openGame) return
  publicToggling.value = true
  try {
    await games.setPublic(games.openGame.id, next)
  } catch (e) {
    loadError.value = String((e as Error)?.message ?? e)
  } finally {
    publicToggling.value = false
  }
}

// Plain PGN download for the anonymous read-only view (issue #213): the
// authenticated GameReviewPanel already offers this alongside the annotated
// export, which needs an engine the anonymous tier doesn't get.
async function onExportPgn() {
  const game = games.openGame
  if (!game) return
  const pgn = await games.exportPgn(false)
  if (pgn != null) downloadText(`game-${game.id}.pgn`, pgn)
}

// Clear the review when a different game is opened so stale data never shows.
watch(
  () => games.openGame?.id,
  (id) => {
    if (review.gameId !== id) review.clear()
  },
)

async function onSelectDatabase() {
  if (selectedDb.value == null) return
  selected.value = new Set()
  await games.selectDatabase(selectedDb.value)
}

// --- URL addressability (issue #212): /games/:id? + ?db= ---------------------

// Selection → URL: mirror the open game and database so the view is deep-
// linkable. `replace` (not `push`) keeps in-page selection out of the history
// stack; the equality guard stops hydration from echoing a redundant navigation.
watch(
  () => [games.openGame?.id, games.databaseId] as const,
  ([id, db]) => {
    if (route.name !== 'games') return
    if (numericParam(route.params.id) === (id ?? null) && numericParam(route.query.db) === (db ?? null)) return
    void router.replace({
      name: 'games',
      params: id != null ? { id: String(id) } : {},
      query: db != null ? { ...route.query, db: String(db) } : route.query,
    })
  },
)

// URL → selection: hydrate on back/forward (mount is handled in onMounted).
watch(
  () => [route.params.id, route.query.db] as const,
  async () => {
    if (route.name !== 'games') return
    const db = numericParam(route.query.db)
    if (db != null && db !== games.databaseId) {
      selectedDb.value = db
      await games.selectDatabase(db)
    }
    const id = numericParam(route.params.id)
    if (id != null && id !== games.openGame?.id) await games.open(id)
  },
)

function onKey(e: KeyboardEvent) {
  if (!games.openGame) return
  const target = e.target as HTMLElement | null
  if (target && (target.tagName === 'TEXTAREA' || target.tagName === 'INPUT')) return
  if (e.key === 'ArrowLeft') {
    games.prev()
    e.preventDefault()
  } else if (e.key === 'ArrowRight') {
    games.next()
    e.preventDefault()
  } else if (e.key === 'ArrowUp' || e.key === 'Home') {
    games.first()
    e.preventDefault()
  } else if (e.key === 'ArrowDown' || e.key === 'End') {
    games.last()
    e.preventDefault()
  }
}

onMounted(async () => {
  window.addEventListener('keydown', onKey)
  api.health().then((h) => (engineEnabled.value = h.engine === true)).catch(() => {})
  const gameId = numericParam(route.params.id)

  // Anonymous (issue #213): the router only let this mount through for a
  // `/games/:id` deep link to a public game. The list/browse endpoints
  // (`databases.list`, `games.list`) require a session, so skip straight to
  // opening the game instead of 401ing on them first.
  if (auth.isAnonymous) {
    if (gameId != null) await games.open(gameId)
    return
  }

  try {
    databases.value = await api.databases.list()
    // Preselect the URL's ?db= (issue #212), else the user's default database,
    // else the first available.
    const urlDb = numericParam(route.query.db)
    const preferred =
      databases.value.find((d) => d.id === urlDb) ??
      databases.value.find((d) => d.id === settings.defaultDatabaseId) ??
      databases.value[0]
    if (preferred) {
      selectedDb.value = preferred.id
      await onSelectDatabase()
    }
    // Deep link straight to a game: /games/:id.
    if (gameId != null) await games.open(gameId)
  } catch (e) {
    loadError.value = String(e)
  }
})

onUnmounted(() => window.removeEventListener('keydown', onKey))
</script>

<template>
  <div class="mx-auto max-w-6xl p-6">
    <header class="mb-4 flex items-center gap-3">
      <h2 class="text-lg font-semibold">
        Games
      </h2>
      <select
        v-if="!auth.isAnonymous"
        v-model="selectedDb"
        class="rounded border border-border px-2 py-1 text-sm"
        aria-label="Database"
        @change="onSelectDatabase"
      >
        <option
          v-for="d in databases"
          :key="d.id"
          :value="d.id"
        >
          {{ d.name }}{{ d.global ? ' (global)' : '' }}
        </option>
      </select>
      <ShareToggle
        v-if="games.openGame && !auth.isAnonymous"
        class="ml-auto"
        :model-value="games.openGame.public"
        :share-url="shareUrl(games.openGame.id)"
        :disabled="publicToggling"
        @update:model-value="onTogglePublic"
      />
    </header>

    <p
      v-if="loadError"
      class="mb-3 text-sm text-bad"
    >
      {{ loadError }}
    </p>

    <!-- Merge selection bar (issue #170): fold the ticked games into one study. -->
    <div
      v-if="selected.size && !auth.isAnonymous"
      class="mb-3 flex items-center gap-3 rounded border border-border bg-surface-2 px-3 py-2 text-sm"
    >
      <span class="text-muted">{{ selected.size }} selected</span>
      <button
        type="button"
        data-test="merge-games"
        class="rounded bg-accent px-3 py-1 font-medium text-surface hover:opacity-90 disabled:opacity-50"
        :disabled="selected.size < 2 || merging"
        @click="mergeSelected"
      >
        {{ merging ? 'Merging…' : 'Merge into repertoire study…' }}
      </button>
      <button
        type="button"
        class="rounded border border-border px-2 py-1"
        @click="selected = new Set()"
      >
        Clear
      </button>
      <span
        v-if="mergeError"
        class="text-bad"
      >{{ mergeError }}</span>
    </div>

    <div class="flex flex-col gap-6 lg:flex-row">
      <!-- Game list (issue #213: needs a session, hidden for an anonymous deep link). -->
      <section
        v-if="!auth.isAnonymous"
        class="lg:w-1/2"
      >
        <table class="w-full text-sm">
          <thead class="text-left text-muted">
            <tr>
              <th class="py-1 pr-2" />
              <th class="py-1 pr-2">
                Players
              </th>
              <th
                class="cursor-pointer select-none py-1 pr-2 hover:text-fg"
                @click="games.setSort('result')"
              >
                Result{{ sortArrow('result') }}
              </th>
              <th
                class="cursor-pointer select-none py-1 pr-2 hover:text-fg"
                @click="games.setSort('date')"
              >
                Date{{ sortArrow('date') }}
              </th>
              <th
                class="cursor-pointer select-none py-1 pr-2 hover:text-fg"
                @click="games.setSort('eco')"
              >
                ECO{{ sortArrow('eco') }}
              </th>
              <th class="py-1" />
            </tr>
          </thead>
          <tbody>
            <tr
              v-for="g in games.games"
              :key="g.id"
              class="group cursor-pointer border-t border-border hover:bg-surface-2"
              :class="{ 'bg-surface-2': games.openGame?.id === g.id }"
              @click="games.open(g.id)"
            >
              <td
                class="py-1 pr-2"
                @click.stop
              >
                <input
                  type="checkbox"
                  data-test="select-game"
                  :aria-label="`Select ${players(g)} for merge`"
                  :checked="selected.has(g.id)"
                  @change="toggleSelected(g.id)"
                >
              </td>
              <td class="py-1 pr-2">
                {{ players(g) }}
              </td>
              <td class="py-1 pr-2">
                {{ g.result ?? '*' }}
              </td>
              <td class="py-1 pr-2">
                {{ g.date ?? '' }}
              </td>
              <td class="py-1 pr-2">
                {{ g.eco ?? '' }}
              </td>
              <td class="py-1 text-right">
                <button
                  type="button"
                  data-test="delete-game"
                  class="rounded px-1.5 text-muted opacity-0 transition hover:text-bad group-hover:opacity-100"
                  title="Delete game"
                  @click.stop="onDelete(g)"
                >
                  ✕
                </button>
              </td>
            </tr>
          </tbody>
        </table>

        <p
          v-if="!games.games.length && !games.loading"
          class="mt-3 text-sm text-muted"
        >
          No games in this database.
        </p>

        <!-- Paginator: prev / page indicator / next, with the row range. -->
        <div
          v-if="games.total > 0"
          class="mt-3 flex items-center gap-3 text-sm"
        >
          <button
            class="rounded border border-border px-2 py-1 disabled:opacity-50"
            :disabled="!games.hasPrev || games.loading"
            aria-label="Previous page"
            @click="games.goToPage(games.page - 1)"
          >
            ◀ Prev
          </button>
          <span class="text-muted">
            Page {{ games.page + 1 }} of {{ games.pageCount }}
          </span>
          <button
            class="rounded border border-border px-2 py-1 disabled:opacity-50"
            :disabled="!games.hasNext || games.loading"
            aria-label="Next page"
            @click="games.goToPage(games.page + 1)"
          >
            Next ▶
          </button>
          <span class="text-muted">
            {{ games.rangeStart }}–{{ games.rangeEnd }} of {{ games.total }}
          </span>
        </div>
      </section>

      <!-- Board viewer -->
      <section
        v-if="games.openGame"
        class="lg:w-1/2"
      >
        <Board
          :fen="games.fen"
          :orientation="games.orientation"
          :dests="games.legalDests"
          :movable="!auth.isAnonymous"
          :last-move="games.lastMove"
          :board-theme="settings.boardTheme"
          :shapes="boardShapes"
          @move="onMove"
        />

        <BoardControls
          class="mt-3"
          :at-start="games.atStart"
          :at-end="games.atEnd"
          @first="games.first()"
          @prev="games.prev()"
          @next="games.next()"
          @last="games.last()"
          @clear-arrows="clearArrows"
        />

        <p class="mt-3 flex items-center gap-2 text-sm font-medium">
          {{ games.openGame.white ?? '?' }} – {{ games.openGame.black ?? '?' }}
          <span class="text-muted">{{ games.openGame.result ?? '*' }}</span>
          <button
            v-if="auth.isAnonymous"
            type="button"
            data-test="export-game"
            class="ml-auto rounded border border-border px-2 py-1 text-xs font-normal hover:bg-surface-2"
            @click="onExportPgn"
          >
            Export PGN
          </button>
        </p>

        <MoveComment
          class="mt-2"
          :tree="games.tree"
          :current-id="games.currentId"
        />

        <MoveTree
          class="mt-2"
          :tree="games.tree"
          :current-id="games.currentId"
          @select="games.goto($event)"
        />

        <template v-if="!auth.isAnonymous">
          <GameReviewPanel
            class="mt-4"
            :engine-enabled="engineEnabled"
          />

          <div class="mt-4">
            <EnginePanel :fen="games.fen" />
          </div>
        </template>
      </section>
    </div>
  </div>
</template>
