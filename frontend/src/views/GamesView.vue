<script setup>
// Game browser (issue #68): pick a database, page through its games, open one on
// the board and step through its moves (buttons, ply selector, arrow keys).
import { computed, onMounted, onUnmounted, ref } from 'vue'
import Board from '../components/Board.vue'
import { api } from '../api.js'
import { useGamesStore } from '../stores/games.js'
import { useSettingsStore } from '../stores/settings.js'

const games = useGamesStore()
const settings = useSettingsStore()

const databases = ref([])
const selectedDb = ref(null)
const loadError = ref(null)

/** A "White – Black" label for a game row, tolerating missing names. */
function players(g) {
  return `${g.white ?? '?'} – ${g.black ?? '?'}`
}

// SAN moves of the open game (ply 1+), for the move list.
const moves = computed(() => games.positions.slice(1).map((p) => p.san))

async function onSelectDatabase() {
  if (selectedDb.value == null) return
  await games.selectDatabase(selectedDb.value)
}

function onKey(e) {
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
              <th class="py-1 pr-2">
                Result
              </th>
              <th class="py-1 pr-2">
                Date
              </th>
              <th class="py-1 pr-2">
                ECO
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

        <button
          v-if="games.hasMore"
          class="mt-3 rounded border border-neutral-300 px-3 py-1 text-sm hover:bg-neutral-100 disabled:opacity-50"
          :disabled="games.loading"
          @click="games.loadMore()"
        >
          {{ games.loading ? 'Loading…' : 'Load more' }}
        </button>
      </section>

      <!-- Board viewer -->
      <section
        v-if="games.openGame"
        class="lg:w-1/2"
      >
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
            @input="games.go(Number($event.target.value))"
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
            class="cursor-pointer rounded px-1"
            :class="{ 'bg-yellow-200': games.ply === i + 1 }"
            @click="games.go(i + 1)"
          >
            <span
              v-if="i % 2 === 0"
              class="text-neutral-400"
            >{{ i / 2 + 1 }}.</span>
            {{ san }}
          </li>
        </ol>
      </section>
    </div>
  </div>
</template>
