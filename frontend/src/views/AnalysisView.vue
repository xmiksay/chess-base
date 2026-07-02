<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref } from 'vue'
import Board from '../components/Board.vue'
import AnalysisPanel from '../components/AnalysisPanel.vue'
import BoardControls from '../components/BoardControls.vue'
import BoardEvalBar from '../components/BoardEvalBar.vue'
import MoveTree from '../components/MoveTree.vue'
import MoveComment from '../components/MoveComment.vue'
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

/** Delete a node (and its line) from the in-memory analysis tree, after confirm. */
function onRemove(id: number) {
  if (window.confirm('Delete this move and everything after it in the line?')) {
    game.removeNode(id)
  }
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
  <div class="mx-auto flex max-w-6xl flex-col gap-6 p-6 md:flex-row md:items-start">
    <section class="flex flex-col gap-4">
      <!-- Stockfish above the board. -->
      <div class="rounded-lg border border-border bg-surface p-4 shadow-sm">
        <AnalysisPanel />
      </div>

      <!-- Eval "thermometer" hugging the board's left edge (matches board height). -->
      <div>
        <div class="flex items-stretch gap-2">
          <BoardEvalBar :fen="game.fen" />
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
        </div>
        <p
          v-if="error"
          class="mt-2 text-sm text-bad"
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

        <MoveComment
          class="mt-3"
          :tree="game.tree"
          :current-id="game.currentId"
        />
      </div>
    </section>

    <aside class="flex w-full flex-1 flex-col gap-4 md:max-w-md">
      <div class="rounded-lg border border-border bg-surface p-4 shadow-sm">
        <h2 class="mb-2 text-xs font-semibold uppercase tracking-wide text-muted">
          Moves
        </h2>
        <!-- Cap the notation to ~board height and scroll in place. -->
        <div class="max-h-[480px] overflow-y-auto">
          <MoveTree
            :tree="game.tree"
            :current-id="game.currentId"
            editable
            @select="game.goto($event)"
            @promote="game.promoteNode($event)"
            @demote="game.demoteNode($event)"
            @remove="onRemove($event)"
          />
        </div>
      </div>
    </aside>
  </div>
</template>
