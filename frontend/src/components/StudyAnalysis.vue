<script setup lang="ts">
// Engine analysis for the study editor: reuses the shared EnginePanel (#134) for
// the eval bar / PV display, driven by the study editor's selected-node `fen`,
// and keeps the study-specific seam — pin an engine line's plan to the current
// node (#61). The two analysis panels live on different routes, so they never
// contend for the singleton engine socket.
import { ref } from 'vue'
import { useEngineStore } from '../stores/engine'
import { useStudyEditorStore } from '../stores/studyEditor'
import { plansToShapes } from '../lib/plansToShapes'
import { useEnginePrefs } from '../lib/useEnginePrefs'
import type { AnalyseStats, EngineLine, Shape } from '../types'
import EnginePanel from './EnginePanel.vue'

const engine = useEngineStore()
const editor = useStudyEditorStore()
const prefs = useEnginePrefs()

const pinError = ref<string | null>(null)

// "Analyse study" bulk pass (#162, full classification #189): walk the engine
// over every node, fill `[%eval]` and classify each move (!/?!/?/??).
const analysing = ref(false)
const analyseError = ref<string | null>(null)
const analyseStats = ref<AnalyseStats | null>(null)

async function analyseStudy() {
  if (analysing.value) return
  analyseError.value = null
  analysing.value = true
  try {
    analyseStats.value = await editor.analyseStudy()
  } catch (e) {
    analyseError.value = String((e as Error)?.message ?? e)
  } finally {
    analysing.value = false
  }
}

// Bulk shape-clear (issue #191): "generated" strips only the plan/threat
// arrows a generate/analyse pass pinned, keeping anything drawn by hand;
// "all" is destructive to hand-drawn shapes too, so it asks first.
const clearing = ref(false)
const clearError = ref<string | null>(null)

async function clearGeneratedShapes() {
  if (clearing.value) return
  clearError.value = null
  clearing.value = true
  try {
    await editor.clearShapes('generated')
  } catch (e) {
    clearError.value = String((e as Error)?.message ?? e)
  } finally {
    clearing.value = false
  }
}

async function clearAllShapes() {
  if (clearing.value) return
  if (!window.confirm('Remove every arrow/highlight on every node, including ones you drew yourself?')) {
    return
  }
  clearError.value = null
  clearing.value = true
  try {
    await editor.clearShapes('all')
  } catch (e) {
    clearError.value = String((e as Error)?.message ?? e)
  } finally {
    clearing.value = false
  }
}

// Transposition pass (#174): append "Transposes to …" comments on nodes whose
// position was already reached earlier in the tree. Comments only — node ids
// are stable, so the selection survives the refresh.
const marking = ref(false)
const markError = ref<string | null>(null)

async function markTranspositions() {
  if (marking.value) return
  markError.value = null
  marking.value = true
  try {
    await editor.markTranspositions()
  } catch (e) {
    markError.value = String((e as Error)?.message ?? e)
  } finally {
    marking.value = false
  }
}

/** "xx.x%" accuracy for the stats summary. */
function pct(n: number): string {
  return `${n.toFixed(1)}%`
}

/** Whether this line has a computed plan to pin (a matching `planline` arrived). */
function planFor(line: EngineLine) {
  return engine.plans.find((p) => p.multipv === line.multipv) ?? null
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
</script>

<template>
  <div
    class="space-y-3"
    data-test="study-analysis"
  >
    <p
      v-if="pinError"
      class="text-sm text-bad"
      data-test="pin-error"
    >
      {{ pinError }}
    </p>

    <!-- Bulk classification pass (#162, #189): writes [%eval] and a
         !/?!/?/?? NAG on every node from the engine. -->
    <div class="space-y-1">
      <button
        type="button"
        data-test="analyse-study"
        class="w-full rounded border border-border px-3 py-1 text-sm hover:bg-surface-2 disabled:opacity-60"
        :disabled="analysing"
        title="Run the engine over every move: store an eval and classify the move played"
        @click="analyseStudy"
      >
        {{ analysing ? 'Analysing…' : 'Analyse study' }}
      </button>
      <p
        v-if="analyseError"
        class="text-sm text-bad"
        data-test="analyse-error"
      >
        {{ analyseError }}
      </p>
      <div
        v-if="analyseStats"
        class="grid grid-cols-2 gap-2 text-xs"
        data-test="analyse-stats"
      >
        <div
          v-for="side in (['white', 'black'] as const)"
          :key="side"
          class="rounded border border-border p-2"
          :data-test="`analyse-stats-${side}`"
        >
          <p class="mb-1 font-medium capitalize">
            {{ side }}
          </p>
          <p>Accuracy: {{ pct(analyseStats.summary[side].accuracy) }}</p>
          <p class="text-muted">
            {{ analyseStats.summary[side].inaccuracies }} ?! ·
            {{ analyseStats.summary[side].mistakes }} ? ·
            {{ analyseStats.summary[side].blunders }} ??
          </p>
        </div>
      </div>
    </div>

    <!-- Bulk shape-clear (issue #191): the counterpart to clearing arrows
         node-by-node — "generated" keeps hand-drawn shapes, "all" doesn't. -->
    <div class="flex gap-2">
      <button
        type="button"
        data-test="clear-generated-shapes"
        class="flex-1 rounded border border-border px-3 py-1 text-sm hover:bg-surface-2 disabled:opacity-60"
        :disabled="clearing"
        title="Remove plan/threat arrows a generate or analyse pass pinned; shapes you drew yourself are kept"
        @click="clearGeneratedShapes"
      >
        Remove generated arrows
      </button>
      <button
        type="button"
        data-test="clear-all-shapes"
        class="flex-1 rounded border border-border px-3 py-1 text-sm hover:bg-surface-2 disabled:opacity-60"
        :disabled="clearing"
        title="Remove every arrow/highlight on every node, including ones you drew yourself"
        @click="clearAllShapes"
      >
        Remove all arrows
      </button>
    </div>
    <p
      v-if="clearError"
      class="text-sm text-bad"
      data-test="clear-shapes-error"
    >
      {{ clearError }}
    </p>

    <!-- Transposition pass (#174): comment-only, safe to re-run any time. -->
    <div class="space-y-1">
      <button
        type="button"
        data-test="mark-transpositions"
        class="w-full rounded border border-border px-3 py-1 text-sm hover:bg-surface-2 disabled:opacity-60"
        :disabled="marking"
        title="Comment every move that transposes into a position already reached earlier in the study"
        @click="markTranspositions"
      >
        {{ marking ? 'Marking…' : 'Mark transpositions' }}
      </button>
      <p
        v-if="markError"
        class="text-sm text-bad"
        data-test="mark-transpositions-error"
      >
        {{ markError }}
      </p>
    </div>

    <!-- Shared eval/PV display, driven by the selected node's position. -->
    <EnginePanel :fen="editor.fen">
      <template #controls>
        <div class="grid grid-cols-3 gap-2 text-xs">
          <label class="flex flex-col gap-1">
            Lines
            <select
              v-model.number="engine.multipv"
              class="rounded border border-border px-1 py-0.5"
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
              class="rounded border border-border px-1 py-0.5"
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
              class="rounded border border-border px-1 py-0.5"
              @change="prefs.persist()"
            >
          </label>
        </div>
      </template>

      <template #line-action="{ line }">
        <button
          v-if="planFor(line)?.trajectories.length"
          class="shrink-0 rounded border border-border px-1.5 py-0.5 text-xs hover:bg-surface-2"
          title="Pin this plan to the current study node"
          data-test="pin-line"
          @click="pinLine(line)"
        >
          📌 Pin
        </button>
      </template>
    </EnginePanel>
  </div>
</template>
