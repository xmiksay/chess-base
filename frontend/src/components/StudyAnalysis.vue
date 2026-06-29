<script setup lang="ts">
// Engine analysis for the study editor: a live eval + PV readout for the
// selected node's position. Mirrors AnalysisPanel's analyse mode but is driven
// by the study editor's `fen` (not the game store) and can pin a line's plan to
// the current node. Shares the singleton engine store; the two panels live on
// different routes so they never run at once.
import { ref, computed, watch, onMounted, onUnmounted } from 'vue'
import { useEngineStore } from '../stores/engine'
import { useStudyEditorStore } from '../stores/studyEditor'
import { formatScore } from '../lib/engineStream'
import { plansToShapes } from '../lib/plansToShapes'
import { uciLineToSan } from '../lib/pv'
import { sideToMove } from '../lib/fen'
import type { EngineLine, Shape } from '../types'
import EvalBar from './EvalBar.vue'

const engine = useEngineStore()
const editor = useStudyEditorStore()

const analyseOn = ref(false)
const pinError = ref<string | null>(null)

// Format against the searched position, not the live node (see AnalysisPanel).
const evalFen = computed(() => engine.analysedFen ?? editor.fen)
const stm = computed(() => sideToMove(evalFen.value))
const topScore = computed(() => engine.lines[0]?.score ?? null)
const evalText = computed(() => formatScore(topScore.value, stm.value))

function lineSan(line: EngineLine) {
  return uciLineToSan(evalFen.value, line.pv, 12).join(' ')
}

function planFor(line: EngineLine) {
  return engine.plans.find((p) => p.multipv === line.multipv) ?? null
}

function startAnalyse() {
  engine.analyse(editor.fen, {})
}

/** Pin an engine line's plan to the open study's current node (#61). */
async function pinLine(line: EngineLine) {
  const plan = planFor(line)
  if (!plan) return
  pinError.value = null
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

// Re-analyse whenever the selected node (its position) changes.
watch(
  () => editor.fen,
  () => analyseOn.value && startAnalyse(),
)

watch(analyseOn, (on) => {
  if (on) startAnalyse()
  else engine.stop()
})

onMounted(() => engine.connect())
onUnmounted(() => {
  engine.stop()
  engine.disconnect()
})
</script>

<template>
  <div
    class="space-y-3"
    data-test="study-analysis"
  >
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
    <p
      v-if="pinError"
      class="text-sm text-red-600"
      data-test="pin-error"
    >
      {{ pinError }}
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

    <label class="flex items-center gap-2 text-sm">
      <input
        v-model="analyseOn"
        type="checkbox"
        data-test="analyse-toggle"
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
        class="flex cursor-default items-center gap-2 rounded px-2 py-1 ring-inset transition"
        :class="engine.activeLine === line.multipv ? 'bg-neutral-200 ring-1 ring-neutral-400' : 'bg-neutral-100'"
        @mouseenter="engine.setActiveLine(line.multipv)"
        @mouseleave="engine.setActiveLine(null)"
      >
        <span class="w-12 shrink-0 font-semibold tabular-nums">
          {{ formatScore(line.score, stm) }}
        </span>
        <span class="flex-1 truncate text-neutral-700">{{ lineSan(line) }}</span>
        <button
          v-if="planFor(line)?.trajectories.length"
          class="shrink-0 rounded border border-neutral-300 px-1.5 py-0.5 text-xs hover:bg-neutral-200"
          title="Pin this plan to the current study node"
          data-test="pin-line"
          @click="pinLine(line)"
        >
          📌 Pin
        </button>
      </li>
      <li
        v-if="!engine.lines.length"
        class="text-xs text-neutral-400"
      >
        No analysis yet.
      </li>
    </ol>
  </div>
</template>
