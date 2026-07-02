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

const props = withDefaults(defineProps<{ fen: string; analyse?: boolean }>(), {
  analyse: true,
})

const engine = useEngineStore()
// Analysis is on by default; it kicks off as soon as the engine reports ready
// (the watch below), so opening the page immediately shows an evaluation.
const analyseOn = ref(true)

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

// Start the first search once the engine connects (it isn't ready at mount, so
// the default-on toggle can't analyse immediately). Only fires the transition
// into ready, not every status change.
watch(
  () => engine.ready,
  (ready) => {
    if (ready && props.analyse && analyseOn.value && !engine.analysing) startAnalyse()
  },
)

// When the consumer takes over the engine (e.g. AnalysisPanel switching to play
// mode), drop our toggle so the two never fight over the socket. The consumer's
// own logic is responsible for stopping/redirecting the engine.
watch(
  () => props.analyse,
  (own) => {
    if (!own) analyseOn.value = false
  },
)

onMounted(() => {
  engine.connect()
  // If the engine is already ready (e.g. another panel connected it), the ready
  // watch won't fire — kick the default-on analysis off here.
  if (engine.ready && props.analyse && analyseOn.value) startAnalyse()
})
onUnmounted(() => {
  engine.stop()
  engine.disconnect()
})
</script>

<template>
  <div
    class="space-y-2"
    data-test="engine-panel"
  >
    <!-- Compact header: status dot + engine name, eval + depth on the right. -->
    <div class="flex items-baseline gap-2">
      <span
        class="inline-block h-2 w-2 shrink-0 self-center rounded-full"
        :class="engine.ready ? 'bg-good' : engine.error ? 'bg-bad' : 'bg-muted'"
      />
      <span class="truncate text-sm font-medium">{{ engine.engineName || 'Engine' }}</span>
      <span class="ml-auto text-lg font-semibold tabular-nums">{{ evalText }}</span>
      <span class="shrink-0 text-xs text-muted">
        d{{ engine.depth ?? '–' }}<span v-if="engine.nps"> · {{ Math.round(engine.nps / 1000) }}kn</span>
      </span>
    </div>

    <p
      v-if="engine.error"
      class="text-xs text-bad"
    >
      {{ engine.error }}
    </p>

    <template v-if="analyse">
      <label class="flex items-center gap-1.5 text-xs">
        <input
          v-model="analyseOn"
          type="checkbox"
          data-test="analyse-toggle"
          :disabled="!engine.ready"
        >
        Analyse
      </label>

      <!-- Per-panel engine config (e.g. MultiPV/Threads/Hash). -->
      <slot name="controls" />

      <!-- PV lines -->
      <ol class="space-y-0.5 text-sm">
        <li
          v-for="line in engine.lines"
          :key="line.multipv"
          class="flex cursor-default items-center gap-2 rounded px-2 py-0.5 ring-inset transition"
          :class="engine.activeLine === line.multipv ? 'bg-surface-2 ring-1 ring-border' : 'bg-surface-2/60'"
          @mouseenter="engine.setActiveLine(line.multipv)"
          @mouseleave="engine.setActiveLine(null)"
        >
          <span class="w-11 shrink-0 text-xs font-semibold tabular-nums">
            {{ formatScore(line.score, stm) }}
          </span>
          <span class="flex-1 truncate text-xs text-fg">{{ lineSan(line) }}</span>
          <slot
            name="line-action"
            :line="line"
          />
        </li>
        <li
          v-if="!engine.lines.length"
          class="text-xs text-muted"
        >
          No analysis yet.
        </li>
      </ol>
    </template>
  </div>
</template>
