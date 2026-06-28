<script setup>
import { ref, computed, watch, onMounted, onUnmounted } from 'vue'
import { useEngineStore } from '../stores/engine.js'
import { useGameStore } from '../stores/game.js'
import { formatScore } from '../lib/engineStream.js'
import { uciLineToSan } from '../lib/pv.js'
import { sideToMove } from '../lib/fen.js'
import EvalBar from './EvalBar.vue'

const engine = useEngineStore()
const game = useGameStore()

// Live analysis toggle (analyse mode) and engine think-time (play mode).
const analyseOn = ref(false)
const thinkMs = ref(800)
// Guards applying a bestmove only when we asked the engine to move.
let engineToMove = false

const stm = computed(() => sideToMove(game.fen))
const topScore = computed(() => engine.lines[0]?.score ?? null)
const evalText = computed(() => formatScore(topScore.value, stm.value))

function lineSan(line) {
  return uciLineToSan(game.fen, line.pv, 12).join(' ')
}

function startAnalyse() {
  engine.analyse(game.fen, {})
}

function requestEngineMove() {
  engineToMove = true
  engine.analyse(game.fen, { limits: { movetime_ms: Number(thinkMs.value) } })
}

function maybeEngineMove() {
  if (game.mode !== 'play' || game.gameOver) return
  if (game.turnColor !== game.playColor) requestEngineMove()
}

// Re-analyse / let the engine reply whenever the position changes.
watch(
  () => game.fen,
  () => {
    if (game.mode === 'analyse') {
      if (analyseOn.value) startAnalyse()
    } else {
      maybeEngineMove()
    }
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

watch(analyseOn, (on) => {
  if (game.mode !== 'analyse') return
  if (on) startAnalyse()
  else engine.stop()
})

watch(
  () => game.mode,
  () => {
    engine.stop()
    analyseOn.value = false
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
  } else if (analyseOn.value) {
    startAnalyse()
  }
}

function setPlayColor(color) {
  game.playColor = color
  game.orientation = color
}

function flip() {
  game.orientation = game.orientation === 'white' ? 'black' : 'white'
}

onMounted(() => engine.connect())
onUnmounted(() => engine.disconnect())
</script>

<template>
  <div class="space-y-4">
    <!-- Engine status / picker -->
    <div class="flex items-center justify-between gap-2">
      <div class="flex items-center gap-2">
        <span
          class="inline-block h-2.5 w-2.5 rounded-full"
          :class="engine.ready ? 'bg-green-500' : engine.error ? 'bg-red-500' : 'bg-neutral-400'"
        />
        <span class="text-sm font-medium">{{ engine.engineName || 'Engine' }}</span>
      </div>
      <select
        v-model="game.mode"
        class="rounded border border-neutral-300 bg-white px-2 py-1 text-sm"
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
      v-if="engine.error"
      class="text-sm text-red-600"
    >
      {{ engine.error }}
    </p>

    <!-- Eval + main board readout -->
    <div class="flex gap-3">
      <EvalBar
        :score="topScore"
        :side-to-move="stm"
      />
      <div class="flex-1 space-y-1">
        <div class="text-2xl font-semibold tabular-nums">
          {{ evalText }}
        </div>
        <div class="text-xs text-neutral-500">
          depth {{ engine.depth ?? '–' }}
          <span v-if="engine.nps"> · {{ Math.round(engine.nps / 1000) }} knps</span>
          <span> · {{ engine.status }}</span>
        </div>
      </div>
    </div>

    <!-- Analyse-mode controls -->
    <div
      v-if="game.mode === 'analyse'"
      class="space-y-3"
    >
      <label class="flex items-center gap-2 text-sm">
        <input
          v-model="analyseOn"
          type="checkbox"
          :disabled="!engine.ready"
        >
        Analyse current position
      </label>
      <div class="grid grid-cols-3 gap-2 text-xs">
        <label class="flex flex-col gap-1">
          Lines
          <select
            v-model.number="engine.multipv"
            class="rounded border border-neutral-300 px-1 py-0.5"
            @change="engine.reconfigure()"
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
            class="rounded border border-neutral-300 px-1 py-0.5"
            @change="engine.reconfigure()"
          >
        </label>
        <label class="flex flex-col gap-1">
          Hash (MB)
          <input
            v-model.number="engine.hash"
            type="number"
            min="1"
            max="4096"
            class="rounded border border-neutral-300 px-1 py-0.5"
            @change="engine.reconfigure()"
          >
        </label>
      </div>

      <!-- PV lines -->
      <ol class="space-y-1 text-sm">
        <li
          v-for="line in engine.lines"
          :key="line.multipv"
          class="flex gap-2 rounded bg-neutral-100 px-2 py-1"
        >
          <span class="w-12 shrink-0 font-semibold tabular-nums">
            {{ formatScore(line.score, stm) }}
          </span>
          <span class="truncate text-neutral-700">{{ lineSan(line) }}</span>
        </li>
        <li
          v-if="!engine.lines.length"
          class="text-xs text-neutral-400"
        >
          No analysis yet.
        </li>
      </ol>
    </div>

    <!-- Play-mode controls -->
    <div
      v-else
      class="space-y-3 text-sm"
    >
      <div class="flex items-center gap-2">
        <span class="text-neutral-500">Play as</span>
        <button
          class="rounded border px-2 py-0.5"
          :class="game.playColor === 'white' ? 'border-neutral-800 bg-neutral-800 text-white' : 'border-neutral-300'"
          @click="setPlayColor('white')"
        >
          White
        </button>
        <button
          class="rounded border px-2 py-0.5"
          :class="game.playColor === 'black' ? 'border-neutral-800 bg-neutral-800 text-white' : 'border-neutral-300'"
          @click="setPlayColor('black')"
        >
          Black
        </button>
      </div>
      <label class="flex items-center gap-2">
        <span class="text-neutral-500">Engine time</span>
        <select
          v-model.number="thinkMs"
          class="rounded border border-neutral-300 px-2 py-0.5"
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
        class="rounded bg-neutral-800 px-3 py-1 text-sm text-white"
        @click="newGame"
      >
        New game
      </button>
      <button
        class="rounded border border-neutral-300 px-3 py-1 text-sm"
        @click="flip"
      >
        Flip
      </button>
    </div>
  </div>
</template>
