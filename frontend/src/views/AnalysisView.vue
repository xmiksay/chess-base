<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref } from 'vue'
import Board from '../components/Board.vue'
import AnalysisPanel from '../components/AnalysisPanel.vue'
import MoveList from '../components/MoveList.vue'
import { useGameStore } from '../stores/game'
import { useSettingsStore } from '../stores/settings'
import { useEngineStore } from '../stores/engine'
import { api } from '../api'
import type { BoardMove } from '../types'

const game = useGameStore()
const settings = useSettingsStore()
const engine = useEngineStore()
const error = ref<string | null>(null)

// In play mode only the human's side may move (and only while the game is live).
const movable = computed(() =>
  game.mode === 'analyse' ? true : game.turnColor === game.playColor && !game.gameOver,
)

function onMove({ from, to }: BoardMove) {
  game.playMove({ from, to })
}

// ← / → step through plies; ↑ / Home jump to start, ↓ / End to the last move.
function onKey(e: KeyboardEvent) {
  const target = e.target as HTMLElement | null
  if (target && (target.tagName === 'TEXTAREA' || target.tagName === 'INPUT')) return
  if (e.key === 'ArrowLeft') {
    game.prev()
    e.preventDefault()
  } else if (e.key === 'ArrowRight') {
    game.next()
    e.preventDefault()
  } else if (e.key === 'ArrowUp' || e.key === 'Home') {
    game.first()
    e.preventDefault()
  } else if (e.key === 'ArrowDown' || e.key === 'End') {
    game.last()
    e.preventDefault()
  }
}

onMounted(async () => {
  window.addEventListener('keydown', onKey)
  try {
    await api.health()
  } catch (e) {
    error.value = String(e)
  }
})

onUnmounted(() => window.removeEventListener('keydown', onKey))
</script>

<template>
  <div class="mx-auto flex max-w-5xl flex-col gap-6 p-6 md:flex-row">
    <section>
      <Board
        :fen="game.fen"
        :orientation="game.orientation"
        :dests="game.legalDests"
        :movable="movable"
        :last-move="game.lastMove"
        :board-theme="settings.boardTheme"
        :shapes="engine.shapes"
        @move="onMove"
      />
      <p
        v-if="error"
        class="mt-2 text-sm text-red-600"
      >
        Backend offline: {{ error }}
      </p>

      <!-- Move-list navigation -->
      <div class="mt-3 flex items-center gap-2">
        <button
          class="rounded border border-neutral-300 px-2 py-1 text-sm disabled:opacity-50"
          :disabled="game.atStart"
          aria-label="Start"
          @click="game.first()"
        >
          ⏮
        </button>
        <button
          class="rounded border border-neutral-300 px-2 py-1 text-sm disabled:opacity-50"
          :disabled="game.atStart"
          aria-label="Back"
          @click="game.prev()"
        >
          ◀
        </button>
        <button
          class="rounded border border-neutral-300 px-2 py-1 text-sm disabled:opacity-50"
          :disabled="game.atEnd"
          aria-label="Forward"
          @click="game.next()"
        >
          ▶
        </button>
        <button
          class="rounded border border-neutral-300 px-2 py-1 text-sm disabled:opacity-50"
          :disabled="game.atEnd"
          aria-label="End"
          @click="game.last()"
        >
          ⏭
        </button>
      </div>

      <MoveList
        class="mt-3"
        :history="game.history"
        :current-ply="game.ply"
        @select="game.goto($event)"
      />
    </section>

    <aside class="flex-1">
      <AnalysisPanel />
    </aside>
  </div>
</template>
