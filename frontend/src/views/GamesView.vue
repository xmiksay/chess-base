<script setup lang="ts">
// Game browser (issue #68): pick a database, page through its games, open one on
// the shared variation-tree board (issue #136) and explore it — step the cursor,
// click moves/variations in the tree, or play an off-line move to branch. The
// engine review (analyze/export, eval graph, why-note) lives in GameReviewPanel.
import { onMounted, onUnmounted, ref, watch } from 'vue'
import Board from '../components/Board.vue'
import BoardControls from '../components/BoardControls.vue'
import MoveTree from '../components/MoveTree.vue'
import EnginePanel from '../components/EnginePanel.vue'
import GameReviewPanel from '../components/GameReviewPanel.vue'
import { api } from '../api'
import { useGamesStore, type GameSortField } from '../stores/games'
import { useReviewStore } from '../stores/review'
import { useSettingsStore } from '../stores/settings'
import { useBoardOverlays } from '../lib/useBoardOverlays'
import type { BoardMove, Database, GameRow } from '../types'

const games = useGamesStore()
const review = useReviewStore()
const settings = useSettingsStore()

const databases = ref<Database[]>([])
const selectedDb = ref<number | null>(null)
const loadError = ref<string | null>(null)
// Engine capability flag from `/api/health`; null until fetched.
const engineEnabled = ref<boolean | null>(null)
const boardRef = ref<InstanceType<typeof Board> | null>(null)

// Composed overlay layers (plans/threats/master) driven by the board's live FEN.
const { boardShapes } = useBoardOverlays(() => games.fen)

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

/** Clear the user's hand-drawn arrows from the board (computed layers stay). */
function clearArrows() {
  boardRef.value?.clearUserShapes()
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
  await games.selectDatabase(selectedDb.value)
}

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
  try {
    databases.value = await api.databases.list()
    // Preselect the user's default database, else the first available.
    const preferred =
      databases.value.find((d) => d.id === settings.defaultDatabaseId) ?? databases.value[0]
    if (preferred) {
      selectedDb.value = preferred.id
      await onSelectDatabase()
    }
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
        v-model="selectedDb"
        class="rounded border border-neutral-300 px-2 py-1 text-sm"
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
    </header>

    <p
      v-if="loadError"
      class="mb-3 text-sm text-red-600"
    >
      {{ loadError }}
    </p>

    <div class="flex flex-col gap-6 lg:flex-row">
      <!-- Game list -->
      <section class="lg:w-1/2">
        <table class="w-full text-sm">
          <thead class="text-left text-neutral-500">
            <tr>
              <th class="py-1 pr-2">
                Players
              </th>
              <th
                class="cursor-pointer select-none py-1 pr-2 hover:text-neutral-800"
                @click="games.setSort('result')"
              >
                Result{{ sortArrow('result') }}
              </th>
              <th
                class="cursor-pointer select-none py-1 pr-2 hover:text-neutral-800"
                @click="games.setSort('date')"
              >
                Date{{ sortArrow('date') }}
              </th>
              <th
                class="cursor-pointer select-none py-1 pr-2 hover:text-neutral-800"
                @click="games.setSort('eco')"
              >
                ECO{{ sortArrow('eco') }}
              </th>
            </tr>
          </thead>
          <tbody>
            <tr
              v-for="g in games.games"
              :key="g.id"
              class="cursor-pointer border-t border-neutral-200 hover:bg-neutral-100"
              :class="{ 'bg-neutral-100': games.openGame?.id === g.id }"
              @click="games.open(g.id)"
            >
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
            </tr>
          </tbody>
        </table>

        <p
          v-if="!games.games.length && !games.loading"
          class="mt-3 text-sm text-neutral-500"
        >
          No games in this database.
        </p>

        <!-- Paginator: prev / page indicator / next, with the row range. -->
        <div
          v-if="games.total > 0"
          class="mt-3 flex items-center gap-3 text-sm"
        >
          <button
            class="rounded border border-neutral-300 px-2 py-1 disabled:opacity-50"
            :disabled="!games.hasPrev || games.loading"
            aria-label="Previous page"
            @click="games.goToPage(games.page - 1)"
          >
            ◀ Prev
          </button>
          <span class="text-neutral-600">
            Page {{ games.page + 1 }} of {{ games.pageCount }}
          </span>
          <button
            class="rounded border border-neutral-300 px-2 py-1 disabled:opacity-50"
            :disabled="!games.hasNext || games.loading"
            aria-label="Next page"
            @click="games.goToPage(games.page + 1)"
          >
            Next ▶
          </button>
          <span class="text-neutral-400">
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
          ref="boardRef"
          :fen="games.fen"
          :orientation="games.orientation"
          :dests="games.legalDests"
          :movable="true"
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

        <p class="mt-3 text-sm font-medium">
          {{ games.openGame.white ?? '?' }} – {{ games.openGame.black ?? '?' }}
          <span class="text-neutral-500">{{ games.openGame.result ?? '*' }}</span>
        </p>

        <MoveTree
          class="mt-2"
          :tree="games.tree"
          :current-id="games.currentId"
          @select="games.goto($event)"
        />

        <GameReviewPanel
          class="mt-4"
          :engine-enabled="engineEnabled"
        />

        <div class="mt-4">
          <EnginePanel :fen="games.fen" />
        </div>
      </section>
    </div>
  </div>
</template>
