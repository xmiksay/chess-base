<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref, watch } from 'vue'
import Board from '../components/Board.vue'
import AnalysisPanel from '../components/AnalysisPanel.vue'
import MoveTree from '../components/MoveTree.vue'
import { useGameStore } from '../stores/game'
import { useSettingsStore } from '../stores/settings'
import { useEngineStore } from '../stores/engine'
import { useOverlaysStore } from '../stores/overlays'
import { composeBoardShapes } from '../lib/boardShapes'
import { api } from '../api'
import type { BoardMove } from '../types'

const game = useGameStore()
const settings = useSettingsStore()
const engine = useEngineStore()
const overlays = useOverlaysStore()
const error = ref<string | null>(null)
const boardRef = ref<InstanceType<typeof Board> | null>(null)

// The board shows the union of the enabled overlay layers (issue #123): the
// engine Plans overlay, the Threats arrows and the database master moves — each
// gated by its persisted setting, composed in one place.
const boardShapes = computed(() =>
  composeBoardShapes(
    { plans: engine.shapes, threats: overlays.threats, master: overlays.master },
    {
      plans: settings.showPlans,
      threats: settings.showThreats,
      master: settings.showMasterMoves,
    },
  ),
)

// (Re)load the position-derived layers when the position or their toggle changes;
// clear a layer the moment it is switched off so stale arrows never linger.
watch(
  [() => game.fen, () => settings.showThreats, () => settings.showMasterMoves],
  () => {
    if (settings.showThreats) overlays.loadThreats(game.fen)
    else overlays.clearThreats()
    if (settings.showMasterMoves) overlays.loadMaster(game.fen)
    else overlays.clearMaster()
  },
  { immediate: true },
)

/** Toggle one overlay layer and persist the choice to user settings. */
function toggleLayer(key: 'showPlans' | 'showThreats' | 'showMasterMoves', value: boolean) {
  settings.update({ [key]: value })
}

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

      <!-- Board-overlay layers (issue #123): independent, persisted toggles plus
           a control to clear hand-drawn arrows. -->
      <div class="mt-3 flex flex-wrap items-center gap-x-4 gap-y-2 text-sm">
        <label class="flex items-center gap-1.5">
          <input
            type="checkbox"
            :checked="settings.showPlans"
            data-test="toggle-plans"
            @change="toggleLayer('showPlans', ($event.target as HTMLInputElement).checked)"
          >
          <span class="text-green-700">Plans</span>
        </label>
        <label class="flex items-center gap-1.5">
          <input
            type="checkbox"
            :checked="settings.showThreats"
            data-test="toggle-threats"
            @change="toggleLayer('showThreats', ($event.target as HTMLInputElement).checked)"
          >
          <span class="text-red-600">Threats</span>
        </label>
        <label class="flex items-center gap-1.5">
          <input
            type="checkbox"
            :checked="settings.showMasterMoves"
            data-test="toggle-master"
            @change="toggleLayer('showMasterMoves', ($event.target as HTMLInputElement).checked)"
          >
          <span class="text-violet-600">Master moves</span>
        </label>
        <button
          class="ml-auto rounded border border-neutral-300 px-2 py-1 text-xs"
          data-test="clear-arrows"
          @click="clearArrows"
        >
          Clear arrows
        </button>
      </div>

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
