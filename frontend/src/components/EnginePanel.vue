<script setup lang="ts">
// Shared engine-analysis display (issue #134): engine status, eval bar +
// depth/status readout, a live "analyse" toggle and the PV line list — all
// driven by a `:fen` prop and the singleton engine store. Reused by Analyse
// (embedded in AnalysisPanel), Studies and Game review.
//
// Consumers inject their per-panel extras via slots:
//   #controls    — engine config (MultiPV/Threads/Hash) rendered under the toggle
//   #line-action — a trailing action per PV line (e.g. the study "pin plan" button)
//
// `analyse` lets a consumer that drives the engine itself (AnalysisPanel's
// play-vs-engine mode) borrow just the eval readout: when false the toggle, the
// config slot and the PV list are hidden and this panel never touches the socket.
import { ref, computed, watch, onMounted, onUnmounted } from 'vue'
import { useEngineStore } from '../stores/engine'
import { formatScore } from '../lib/engineStream'
import { uciLineToSan } from '../lib/pv'
import { sideToMove } from '../lib/fen'
import type { EngineLine } from '../types'
import EvalBar from './EvalBar.vue'

const props = withDefaults(defineProps<{ fen: string; analyse?: boolean }>(), {
  analyse: true,
})

const engine = useEngineStore()
const analyseOn = ref(false)

// Format eval/PV against the position the engine actually searched, not the live
// board (see the note in stores/engine): in play mode the board moves on after
// the engine replies, and flipping by the new side-to-move would invert the eval.
const evalFen = computed(() => engine.analysedFen ?? props.fen)
const stm = computed(() => sideToMove(evalFen.value))
const topScore = computed(() => engine.lines[0]?.score ?? null)
const evalText = computed(() => formatScore(topScore.value, stm.value))

function lineSan(line: EngineLine) {
  return uciLineToSan(evalFen.value, line.pv, 12).join(' ')
}

function startAnalyse() {
  engine.analyse(props.fen, {})
}

// Re-analyse whenever the position changes (only while this panel owns analysis).
watch(
  () => props.fen,
  () => props.analyse && analyseOn.value && startAnalyse(),
)

watch(analyseOn, (on) => {
  if (!props.analyse) return
  if (on) startAnalyse()
  else engine.stop()
})

// When the consumer takes over the engine (e.g. AnalysisPanel switching to play
// mode), drop our toggle so the two never fight over the socket. The consumer's
// own logic is responsible for stopping/redirecting the engine.
watch(
  () => props.analyse,
  (own) => {
    if (!own) analyseOn.value = false
  },
)

onMounted(() => engine.connect())
onUnmounted(() => {
  engine.stop()
  engine.disconnect()
})
</script>

<template>
  <div
    class="space-y-3"
    data-test="engine-panel"
  >
    <!-- Engine status -->
    <div class="flex items-center gap-2">
      <span
        class="inline-block h-2.5 w-2.5 rounded-full"
        :class="engine.ready ? 'bg-green-500' : engine.error ? 'bg-red-500' : 'bg-neutral-400'"
      />
      <span class="text-sm font-medium">{{ engine.engineName || 'Engine' }}</span>
    </div>

    <p
      v-if="engine.error"
      class="text-sm text-red-600"
    >
      {{ engine.error }}
    </p>

    <!-- Eval readout -->
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

    <template v-if="analyse">
      <label class="flex items-center gap-2 text-sm">
        <input
          v-model="analyseOn"
          type="checkbox"
          data-test="analyse-toggle"
          :disabled="!engine.ready"
        >
        Analyse current position
      </label>

      <!-- Per-panel engine config (e.g. MultiPV/Threads/Hash). -->
      <slot name="controls" />

      <!-- PV lines -->
      <ol class="space-y-1 text-sm">
        <li
          v-for="line in engine.lines"
          :key="line.multipv"
          class="flex cursor-default items-center gap-2 rounded px-2 py-1 ring-inset transition"
          :class="engine.activeLine === line.multipv ? 'bg-neutral-200 ring-1 ring-neutral-400' : 'bg-neutral-100'"
          @mouseenter="engine.setActiveLine(line.multipv)"
          @mouseleave="engine.setActiveLine(null)"
        >
          <span class="w-12 shrink-0 font-semibold tabular-nums">
            {{ formatScore(line.score, stm) }}
          </span>
          <span class="flex-1 truncate text-neutral-700">{{ lineSan(line) }}</span>
          <slot
            name="line-action"
            :line="line"
          />
        </li>
        <li
          v-if="!engine.lines.length"
          class="text-xs text-neutral-400"
        >
          No analysis yet.
        </li>
      </ol>
    </template>
  </div>
</template>
