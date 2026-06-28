<script setup>
import { computed, onMounted, ref } from 'vue'
import Board from './components/Board.vue'
import AnalysisPanel from './components/AnalysisPanel.vue'
import SettingsView from './components/SettingsView.vue'
import { useGameStore } from './stores/game.js'
import { useSettingsStore } from './stores/settings.js'
import { api } from './api.js'

const game = useGameStore()
const settings = useSettingsStore()
const backend = ref(null)
const error = ref(null)
const showSettings = ref(false)

// In play mode only the human's side may move (and only while the game is live).
const movable = computed(() =>
  game.mode === 'analyse' ? true : game.turnColor === game.playColor && !game.gameOver,
)

function onMove({ from, to }) {
  game.playMove({ from, to })
}

onMounted(async () => {
  settings.load()
  try {
    backend.value = await api.health()
  } catch (e) {
    error.value = String(e)
  }
})
</script>

<template>
  <div class="min-h-screen bg-neutral-50 text-neutral-900">
    <header class="border-b border-neutral-200 px-6 py-4">
      <div class="flex items-center justify-between">
        <h1 class="text-xl font-semibold">
          chess-base
        </h1>
        <button
          class="text-sm text-neutral-600 hover:underline"
          @click="showSettings = !showSettings"
        >
          {{ showSettings ? 'Close settings' : 'Settings' }}
        </button>
      </div>
      <p class="text-sm text-neutral-500">
        Collect, store, search and study chess games.
      </p>
    </header>

    <div
      v-if="showSettings"
      class="mx-auto max-w-5xl p-6"
    >
      <SettingsView />
    </div>

    <main class="mx-auto flex max-w-5xl flex-col gap-6 p-6 md:flex-row">
      <section>
        <Board
          :fen="game.fen"
          :orientation="game.orientation"
          :dests="game.legalDests"
          :movable="movable"
          :board-theme="settings.boardTheme"
          @move="onMove"
        />
        <p
          v-if="error"
          class="mt-2 text-sm text-red-600"
        >
          Backend offline: {{ error }}
        </p>
      </section>

      <aside class="flex-1">
        <AnalysisPanel />
      </aside>
    </main>
  </div>
</template>
