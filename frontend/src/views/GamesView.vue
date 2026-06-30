<script setup lang="ts">
// Game browser (issue #68): pick a database, page through its games, open one on
// the board and step through its moves (buttons, ply selector, arrow keys).
import { computed, onMounted, onUnmounted, ref, watch } from 'vue'
import Board from '../components/Board.vue'
import EvalGraph from '../components/EvalGraph.vue'
import { api } from '../api'
import { downloadText } from '../lib/download'
import { useGamesStore, type GameSortField } from '../stores/games'
import { useReviewStore } from '../stores/review'
import { useSettingsStore } from '../stores/settings'
import {
  classificationClass,
  classificationGlyph,
  formatReviewEval,
} from '../lib/reviewFormat'
import type { Database, GameRow } from '../types'

const games = useGamesStore()
const review = useReviewStore()
const settings = useSettingsStore()

const databases = ref<Database[]>([])
const selectedDb = ref<number | null>(null)
const loadError = ref<string | null>(null)
// Engine capability flag from `/api/health`; null until fetched.
const engineEnabled = ref<boolean | null>(null)

/** A "White – Black" label for a game row, tolerating missing names. */
function players(g: GameRow): string {
  return `${g.white ?? '?'} – ${g.black ?? '?'}`
}

/** The sort-direction arrow for a column header, blank when it isn't active. */
function sortArrow(field: GameSortField): string {
  if (games.sort !== field) return ''
  return games.dir === 'asc' ? ' ▲' : ' ▼'
}

// SAN moves of the open game (ply 1+), for the move list.
const moves = computed(() => games.positions.slice(1).map((p) => p.san))

// The reviewed move at the currently selected ply (for the why-note), if any.
const currentMove = computed(() => review.byPly.get(games.ply) ?? null)

/** White accuracy first, formatted as "xx.x%". */
function pct(n: number): string {
  return `${n.toFixed(1)}%`
}

async function onAnalyse() {
  if (!games.openGame) return
  await review.analyse(games.openGame.id)
}

// Export the open game as a `.pgn` download (issue #120): verbatim, or — with
// `annotated` — carrying the engine analysis (`[%eval]` + NAGs + why-notes).
async function onExport(annotated: boolean) {
  const game = games.openGame
  if (!game) return
  const pgn = await games.exportPgn(annotated)
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
  await games.selectDatabase(selectedDb.value)
}

function onKey(e: KeyboardEvent) {
  if (!games.openGame) return
  if (e.key === 'ArrowLeft') {
    games.go('prev')
    e.preventDefault()
  } else if (e.key === 'ArrowRight') {
    games.go('next')
    e.preventDefault()
  } else if (e.key === 'ArrowUp' || e.key === 'Home') {
    games.go('first')
    e.preventDefault()
  } else if (e.key === 'ArrowDown' || e.key === 'End') {
    games.go('last')
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
        <div class="mb-3 flex items-center gap-2">
          <button
            type="button"
            data-test="analyse"
            class="rounded bg-neutral-800 px-3 py-1 text-sm text-white hover:bg-neutral-700 disabled:opacity-50"
            :disabled="review.loading || engineEnabled === false"
            :title="engineEnabled === false ? 'No engine configured on the server.' : ''"
            @click="onAnalyse"
          >
            {{ review.loading ? 'Analyzing…' : 'Analyze game' }}
          </button>
          <button
            type="button"
            data-test="export"
            class="rounded border border-neutral-300 px-3 py-1 text-sm hover:bg-neutral-100"
            @click="onExport(false)"
          >
            Export PGN
          </button>
          <button
            type="button"
            data-test="export-annotated"
            class="rounded border border-neutral-300 px-3 py-1 text-sm hover:bg-neutral-100 disabled:opacity-50"
            :disabled="engineEnabled === false"
            :title="engineEnabled === false ? 'No engine configured on the server.' : ''"
            @click="onExport(true)"
          >
            Export with analysis
          </button>
          <span
            v-if="engineEnabled === false"
            class="text-xs text-neutral-500"
          >
            No engine configured.
          </span>
          <span
            v-if="review.error"
            class="text-xs text-red-600"
            data-test="review-error"
          >
            {{ review.error }}
          </span>
        </div>

        <Board
          :fen="games.fen"
          :last-move="games.lastMove"
          :board-theme="settings.boardTheme"
        />

        <div class="mt-3 flex items-center gap-2">
          <button
            class="rounded border border-neutral-300 px-2 py-1 text-sm disabled:opacity-50"
            :disabled="games.atStart"
            aria-label="First move"
            @click="games.go('first')"
          >
            ⏮
          </button>
          <button
            class="rounded border border-neutral-300 px-2 py-1 text-sm disabled:opacity-50"
            :disabled="games.atStart"
            aria-label="Previous move"
            @click="games.go('prev')"
          >
            ◀
          </button>
          <input
            type="range"
            min="0"
            :max="games.positions.length - 1"
            :value="games.ply"
            class="flex-1"
            aria-label="Ply"
            @input="games.go(Number(($event.target as HTMLInputElement).value))"
          >
          <button
            class="rounded border border-neutral-300 px-2 py-1 text-sm disabled:opacity-50"
            :disabled="games.atEnd"
            aria-label="Next move"
            @click="games.go('next')"
          >
            ▶
          </button>
          <button
            class="rounded border border-neutral-300 px-2 py-1 text-sm disabled:opacity-50"
            :disabled="games.atEnd"
            aria-label="Last move"
            @click="games.go('last')"
          >
            ⏭
          </button>
        </div>

        <p class="mt-2 text-sm font-medium">
          {{ games.openGame.white ?? '?' }} – {{ games.openGame.black ?? '?' }}
          <span class="text-neutral-500">{{ games.openGame.result ?? '*' }}</span>
        </p>

        <ol class="mt-2 flex flex-wrap gap-x-2 gap-y-1 text-sm">
          <li
            v-for="(san, i) in moves"
            :key="i"
            data-test="move"
            class="cursor-pointer rounded px-1"
            :class="[
              { 'bg-yellow-200': games.ply === i + 1 },
              review.byPly.get(i + 1) ? classificationClass(review.byPly.get(i + 1)!.classification) : '',
            ]"
            @click="games.go(i + 1)"
          >
            <span
              v-if="i % 2 === 0"
              class="text-neutral-400"
            >{{ i / 2 + 1 }}.</span>
            {{ san }}<span
              v-if="review.byPly.get(i + 1)"
              class="font-semibold"
            >{{ classificationGlyph(review.byPly.get(i + 1)!.classification) }}</span>
          </li>
        </ol>

        <!-- Engine review: graph, accuracy summary, and the selected-ply note. -->
        <div
          v-if="review.review"
          class="mt-4"
          data-test="review-panel"
        >
          <EvalGraph
            :moves="review.review.moves"
            :current-ply="games.ply"
            @select="games.go($event)"
          />

          <div class="mt-3 grid grid-cols-2 gap-3 text-xs">
            <div
              v-for="side in (['white', 'black'] as const)"
              :key="side"
              class="rounded border border-neutral-200 p-2"
              :data-test="`summary-${side}`"
            >
              <p class="mb-1 font-medium capitalize">
                {{ side }}
              </p>
              <p>Accuracy: {{ pct(review.review.summary[side].accuracy) }}</p>
              <p>ACPL: {{ review.review.summary[side].acpl }}</p>
              <p class="text-neutral-500">
                {{ review.review.summary[side].inaccuracies }} inacc ·
                {{ review.review.summary[side].mistakes }} mist ·
                {{ review.review.summary[side].blunders }} blun
              </p>
            </div>
          </div>

          <div
            v-if="currentMove"
            class="mt-3 rounded border border-neutral-200 p-2 text-sm"
            data-test="why-note"
          >
            <span
              class="font-medium"
              :class="classificationClass(currentMove.classification)"
            >
              {{ currentMove.san }}{{ classificationGlyph(currentMove.classification) }}
            </span>
            <span class="text-neutral-500"> {{ formatReviewEval(currentMove) }}</span>
            <span
              v-if="currentMove.best_move"
              class="text-neutral-500"
            > · best: {{ currentMove.best_move }}</span>
            <p class="mt-1 text-neutral-700">
              {{ currentMove.explanation }}
            </p>
          </div>
        </div>
      </section>
    </div>
  </div>
</template>
