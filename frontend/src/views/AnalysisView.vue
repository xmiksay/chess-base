<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref } from 'vue'
import Board from '../components/Board.vue'
import AnalysisPanel from '../components/AnalysisPanel.vue'
import BoardControls from '../components/BoardControls.vue'
import MoveTree from '../components/MoveTree.vue'
import { useGameStore } from '../stores/game'
import { useSettingsStore } from '../stores/settings'
import { useBoardOverlays } from '../lib/useBoardOverlays'
import { api } from '../api'
import type { BoardMove } from '../types'

const game = useGameStore()
const settings = useSettingsStore()
const error = ref<string | null>(null)
const boardRef = ref<InstanceType<typeof Board> | null>(null)

// Composed overlay layers (plans/threats/master) driven by the board's live FEN.
const { boardShapes } = useBoardOverlays(() => game.fen)

/** Clear the user's hand-drawn arrows from the board (computed layers stay). */
function clearArrows() {
  boardRef.value?.clearUserShapes()
}

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
        ref="boardRef"
        :fen="game.fen"
        :orientation="game.orientation"
        :dests="game.legalDests"
        :movable="movable"
        :last-move="game.lastMove"
        :board-theme="settings.boardTheme"
        :shapes="boardShapes"
        @move="onMove"
      />
      <p
        v-if="error"
        class="mt-2 text-sm text-red-600"
      >
        Backend offline: {{ error }}
      </p>

      <BoardControls
        class="mt-3"
        :at-start="game.atStart"
        :at-end="game.atEnd"
        @first="game.first()"
        @prev="game.prev()"
        @next="game.next()"
        @last="game.last()"
        @clear-arrows="clearArrows"
      />

      <MoveTree
        class="mt-3"
        :tree="game.tree"
        :current-id="game.currentId"
        @select="game.goto($event)"
      />
    </section>

    <aside class="flex-1">
      <AnalysisPanel />
    </aside>
  </div>
</template>
