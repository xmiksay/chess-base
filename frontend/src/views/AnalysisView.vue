<script setup>
import { computed, onMounted, ref } from 'vue'
import Board from '../components/Board.vue'
import AnalysisPanel from '../components/AnalysisPanel.vue'
import { useGameStore } from '../stores/game.js'
import { useSettingsStore } from '../stores/settings.js'
import { api } from '../api.js'

const game = useGameStore()
const settings = useSettingsStore()
const error = ref(null)

// In play mode only the human's side may move (and only while the game is live).
const movable = computed(() =>
  game.mode === 'analyse' ? true : game.turnColor === game.playColor && !game.gameOver,
)

function onMove({ from, to }) {
  game.playMove({ from, to })
}

onMounted(async () => {
  try {
    await api.health()
  } catch (e) {
    error.value = String(e)
  }
})
</script>

<template>
  <div class="mx-auto flex max-w-5xl flex-col gap-6 p-6 md:flex-row">
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
  </div>
</template>
