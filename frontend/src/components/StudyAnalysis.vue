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
import type { EngineLine, Shape } from '../types'
import EnginePanel from './EnginePanel.vue'

const engine = useEngineStore()
const editor = useStudyEditorStore()

const pinError = ref<string | null>(null)

// "Analyse study" bulk pass (#162): walk the engine over every node and fill
// `[%eval]` so the exported PGN carries evals Lichess renders.
const analysing = ref(false)
const analyseError = ref<string | null>(null)

async function analyseStudy() {
  if (analysing.value) return
  analyseError.value = null
  analysing.value = true
  try {
    await editor.analyseStudy()
  } catch (e) {
    analyseError.value = String((e as Error)?.message ?? e)
  } finally {
    analysing.value = false
  }
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
      class="text-sm text-red-600"
      data-test="pin-error"
    >
      {{ pinError }}
    </p>

    <!-- Bulk "fill evals" pass (#162): writes [%eval] on every node so the
         exported PGN carries evals Lichess renders. -->
    <div class="space-y-1">
      <button
        type="button"
        data-test="analyse-study"
        class="w-full rounded border border-neutral-300 px-3 py-1 text-sm hover:bg-neutral-100 disabled:opacity-60"
        :disabled="analysing"
        title="Run the engine over every move and store a White-perspective eval on each node"
        @click="analyseStudy"
      >
        {{ analysing ? 'Analysing…' : 'Analyse study (fill evals)' }}
      </button>
      <p
        v-if="analyseError"
        class="text-sm text-red-600"
        data-test="analyse-error"
      >
        {{ analyseError }}
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
      </template>

      <template #line-action="{ line }">
        <button
          v-if="planFor(line)?.trajectories.length"
          class="shrink-0 rounded border border-neutral-300 px-1.5 py-0.5 text-xs hover:bg-neutral-200"
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
