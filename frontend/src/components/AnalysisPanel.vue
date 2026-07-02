<script setup lang="ts">
import { ref, computed, watch } from 'vue'
import { useEngineStore } from '../stores/engine'
import { useGameStore } from '../stores/game'
import { useStudyEditorStore } from '../stores/studyEditor'
import { plansToShapes } from '../lib/plansToShapes'
import { useEnginePrefs } from '../lib/useEnginePrefs'
import type { Color, EngineLine, Shape } from '../types'
import EnginePanel from './EnginePanel.vue'

const engine = useEngineStore()
const game = useGameStore()
const editor = useStudyEditorStore()
const prefs = useEnginePrefs()

// Engine think-time (play mode).
const thinkMs = ref(800)
// Guards applying a bestmove only when we asked the engine to move.
let engineToMove = false

// A plan can only be pinned when a study is open (its current node is the target).
const canPin = computed(() => editor.studyId != null)
const pinError = ref<string | null>(null)

/** Whether this line has a computed plan to pin (a matching `planline` arrived). */
function planFor(line: EngineLine) {
  return engine.plans.find((p) => p.multipv === line.multipv) ?? null
}

/** Pin an engine line's plan to the open study's current node (#61). */
async function pinLine(line: EngineLine) {
  const plan = planFor(line)
  if (!canPin.value || !plan) return
  pinError.value = null
  // Reuse the overlay's trajectory→arrow mapping, then keep just the stored
  // shape fields (orig/dest/brush) the backend persists.
  const shapes: Shape[] = plansToShapes([plan]).map((s) => ({
    orig: s.orig,
    dest: s.dest ?? null,
    brush: s.brush ?? 'plan1',
  }))
  try {
    await editor.setShapes(shapes)
  } catch (e) {
    pinError.value = String((e as Error)?.message ?? e)
  }
}

function requestEngineMove() {
  engineToMove = true
  engine.analyse(game.fen, { limits: { movetime_ms: Number(thinkMs.value) } })
}

function maybeEngineMove() {
  if (game.mode !== 'play' || game.gameOver) return
  if (game.turnColor !== game.playColor) requestEngineMove()
}

// Let the engine reply whenever the position changes in play mode (analyse-mode
// re-analysis is owned by the embedded EnginePanel).
watch(
  () => game.fen,
  () => {
    if (game.mode === 'play') maybeEngineMove()
  },
)

// Apply the engine's chosen move in play mode.
watch(
  () => engine.bestMove,
  (bm) => {
    if (bm && game.mode === 'play' && engineToMove) {
      engineToMove = false
      game.playUci(bm.move)
    }
  },
)

watch(
  () => game.mode,
  () => {
    engine.stop()
    engineToMove = false
    if (game.mode === 'play') maybeEngineMove()
  },
)

function newGame() {
  engine.stop()
  engineToMove = false
  game.reset()
  if (game.mode === 'play') {
    game.orientation = game.playColor
    maybeEngineMove()
  }
  // In analyse mode the embedded EnginePanel re-analyses the new position itself.
}

function setPlayColor(color: Color) {
  game.playColor = color
  game.orientation = color
}

function flip() {
  game.orientation = game.orientation === 'white' ? 'black' : 'white'
}
</script>

<template>
  <div class="space-y-4">
    <!-- Mode picker -->
    <div class="flex items-center justify-end">
      <select
        v-model="game.mode"
        class="rounded border border-border bg-surface px-2 py-1 text-sm"
      >
        <option value="analyse">
          Analyse
        </option>
        <option value="play">
          Play vs engine
        </option>
      </select>
    </div>

    <p
      v-if="pinError"
      class="text-sm text-bad"
      data-test="pin-error"
    >
      {{ pinError }}
    </p>

    <!-- Shared engine display; analysis is owned here only in analyse mode. -->
    <EnginePanel
      :fen="game.fen"
      :analyse="game.mode === 'analyse'"
    >
      <template #controls>
        <div class="grid grid-cols-3 gap-2 text-xs">
          <label class="flex flex-col gap-1">
            Lines
            <select
              v-model.number="engine.multipv"
              class="rounded border border-border bg-surface px-1 py-0.5"
              @change="prefs.persist()"
            >
              <option
                v-for="n in 5"
                :key="n"
                :value="n"
              >
                {{ n }}
              </option>
            </select>
          </label>
          <label class="flex flex-col gap-1">
            Threads
            <input
              v-model.number="engine.threads"
              type="number"
              min="1"
              max="64"
              class="rounded border border-border bg-surface px-1 py-0.5"
              @change="prefs.persist()"
            >
          </label>
          <label class="flex flex-col gap-1">
            Hash (MB)
            <input
              v-model.number="engine.hash"
              type="number"
              min="1"
              max="4096"
              class="rounded border border-border bg-surface px-1 py-0.5"
              @change="prefs.persist()"
            >
          </label>
        </div>
      </template>

      <template #line-action="{ line }">
        <button
          v-if="canPin && planFor(line)?.trajectories.length"
          class="shrink-0 rounded border border-border px-1.5 py-0.5 text-xs hover:bg-surface-2"
          title="Pin this plan to the current study node"
          data-test="pin-line"
          @click="pinLine(line)"
        >
          📌 Pin
        </button>
      </template>
    </EnginePanel>

    <!-- Play-mode controls -->
    <div
      v-if="game.mode === 'play'"
      class="space-y-3 text-sm"
    >
      <div class="flex items-center gap-2">
        <span class="text-muted">Play as</span>
        <button
          class="rounded border px-2 py-0.5"
          :class="game.playColor === 'white' ? 'border-fg bg-fg text-surface' : 'border-border'"
          @click="setPlayColor('white')"
        >
          White
        </button>
        <button
          class="rounded border px-2 py-0.5"
          :class="game.playColor === 'black' ? 'border-fg bg-fg text-surface' : 'border-border'"
          @click="setPlayColor('black')"
        >
          Black
        </button>
      </div>
      <label class="flex items-center gap-2">
        <span class="text-muted">Engine time</span>
        <select
          v-model.number="thinkMs"
          class="rounded border border-border px-2 py-0.5"
        >
          <option :value="200">
            Fast
          </option>
          <option :value="800">
            Normal
          </option>
          <option :value="2500">
            Slow
          </option>
        </select>
      </label>
      <p
        v-if="game.result"
        class="font-medium"
      >
        {{ game.result === 'draw' ? 'Draw.' : `${game.result === 'white' ? 'White' : 'Black'} wins.` }}
      </p>
    </div>

    <div class="flex gap-2">
      <button
        class="rounded bg-fg px-3 py-1 text-sm text-surface hover:opacity-90"
        @click="newGame"
      >
        New game
      </button>
      <button
        class="rounded border border-border px-3 py-1 text-sm"
        @click="flip"
      >
        Flip
      </button>
    </div>
  </div>
</template>
