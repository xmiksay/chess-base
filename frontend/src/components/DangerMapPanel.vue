<script setup lang="ts">
// Engine-only danger overlay panel (issue #156, ADR-0026 / ADR-0027). The
// lightweight sibling of GenerateDangerMapDialog: instead of an LLM-annotated
// study, it walks a repertoire spine for danger and renders the raw classifier
// output — Weapon / Caution / Off-book — as board arrows (driven by the parent
// from the danger store) plus this side panel of tagged moves with their figures.
// Needs only an engine, so it works on a local / no-key install.
import { ref } from 'vue'
import { api } from '../api'
import { useDangerStore } from '../stores/danger'
import { STARTPOS_FEN } from '../lib/fen'
import type { DangerWalkBody } from '../types'

interface Props {
  /** The danger walk needs an engine (no LLM). Gates the run button + hint. */
  engineEnabled: boolean
  /** Open study id, if any — enables "use this study's mainline" as the spine. */
  studyId?: number | null
  /** Start position for the walk (e.g. the open study's root); defaults to startpos. */
  startFen?: string
}
const props = withDefaults(defineProps<Props>(), { studyId: null, startFen: STARTPOS_FEN })

const danger = useDangerStore()

const spinePgn = ref('')
const ourSide = ref<'White' | 'Black'>('White')
const maxDepth = ref(8)
const movetimeMs = ref(500)
const multipv = ref(2)
const loadError = ref<string | null>(null)

/** Prefill the spine from the open study's mainline (a plain `.pgn` export). */
async function useStudyMainline() {
  if (props.studyId == null) return
  loadError.value = null
  try {
    spinePgn.value = (await api.studies.exportPgn(props.studyId, { eval: false })).trim()
  } catch (e) {
    loadError.value = String((e as Error)?.message ?? e)
  }
}

async function onShow() {
  if (!props.engineEnabled || !spinePgn.value.trim() || danger.loading) return
  const body: DangerWalkBody = {
    spine_pgn: spinePgn.value.trim(),
    fen: (props.startFen || STARTPOS_FEN).trim(),
    spine: { our_side: ourSide.value, max_depth: maxDepth.value },
    movetime_ms: movetimeMs.value,
    multipv: multipv.value,
  }
  await danger.load(body)
}
</script>

<template>
  <section
    data-test="danger-panel"
    class="rounded border border-neutral-200 p-3"
  >
    <header class="mb-2 flex items-center gap-2">
      <h3 class="text-sm font-semibold">
        Danger map
      </h3>
      <span class="text-xs text-neutral-400">engine only — no AI key needed</span>
      <button
        v-if="danger.tree"
        type="button"
        data-test="danger-clear"
        class="ml-auto rounded border border-neutral-300 px-2 py-0.5 text-xs hover:bg-neutral-100"
        @click="danger.clear()"
      >
        Clear
      </button>
    </header>

    <label class="flex flex-col gap-1 text-sm">
      Repertoire spine (PGN)
      <textarea
        v-model="spinePgn"
        data-test="danger-spine"
        rows="2"
        placeholder="1. e4 c5 2. d4 cxd4 3. c3 *"
        class="rounded border border-neutral-300 px-2 py-1 font-mono text-xs"
      />
    </label>

    <div class="mt-2 grid grid-cols-2 gap-2 text-xs sm:grid-cols-4">
      <label class="flex flex-col gap-1">
        Our side
        <select
          v-model="ourSide"
          data-test="danger-side"
          class="rounded border border-neutral-300 px-2 py-1"
        >
          <option value="White">
            White
          </option>
          <option value="Black">
            Black
          </option>
        </select>
      </label>
      <label class="flex flex-col gap-1">
        Depth
        <input
          v-model.number="maxDepth"
          data-test="danger-depth"
          type="number"
          min="1"
          class="rounded border border-neutral-300 px-2 py-1"
        >
      </label>
      <label class="flex flex-col gap-1">
        Movetime
        <input
          v-model.number="movetimeMs"
          type="number"
          min="1"
          class="rounded border border-neutral-300 px-2 py-1"
        >
      </label>
      <label class="flex flex-col gap-1">
        MultiPV
        <input
          v-model.number="multipv"
          type="number"
          min="2"
          class="rounded border border-neutral-300 px-2 py-1"
        >
      </label>
    </div>

    <div class="mt-2 flex items-center gap-2">
      <button
        type="button"
        data-test="danger-show"
        class="rounded bg-neutral-800 px-3 py-1 text-sm text-white hover:bg-neutral-700 disabled:opacity-50"
        :disabled="!engineEnabled || !spinePgn.trim() || danger.loading"
        @click="onShow"
      >
        {{ danger.loading ? 'Walking…' : 'Show danger' }}
      </button>
      <button
        v-if="studyId != null"
        type="button"
        data-test="danger-mainline"
        class="rounded border border-neutral-300 px-3 py-1 text-sm hover:bg-neutral-100"
        @click="useStudyMainline"
      >
        Use study mainline
      </button>
    </div>

    <p
      v-if="!engineEnabled"
      data-test="danger-engine-hint"
      class="mt-2 text-xs text-neutral-500"
    >
      Configure an engine (--engine / CHESS_BASE_ENGINE) to walk for danger.
    </p>
    <p
      v-if="loadError || danger.error"
      data-test="danger-error"
      class="mt-2 text-xs text-red-600"
    >
      {{ loadError || danger.error }}
    </p>

    <!-- Result digest: legend + the tagged moves with their figures. -->
    <div
      v-if="danger.tree"
      class="mt-3"
    >
      <div class="mb-2 flex flex-wrap gap-3 text-xs text-neutral-500">
        <span><span class="inline-block h-2 w-3 rounded-sm bg-green-700" /> Weapon</span>
        <span><span class="inline-block h-2 w-3 rounded-sm bg-red-600" /> Caution</span>
        <span><span class="inline-block h-2 w-3 rounded-sm bg-amber-600" /> Off-book</span>
      </div>
      <p
        v-if="!danger.roles.length"
        data-test="danger-empty"
        class="text-xs text-neutral-500"
      >
        No dangerous replies flagged on this spine.
      </p>
      <ul
        v-else
        data-test="danger-roles"
        class="max-h-56 space-y-1 overflow-y-auto text-xs"
      >
        <li
          v-for="r in danger.roles"
          :key="r.nodeId"
          class="flex items-center justify-between gap-2 rounded bg-neutral-50 px-2 py-1"
        >
          <span class="font-mono">{{ r.san ?? '—' }}</span>
          <span class="flex items-center gap-2 text-neutral-500">
            <span>{{ r.kind }} · {{ r.role }}</span>
            <span
              v-if="r.onlyMoveGap != null"
              class="text-neutral-400"
            >gap {{ r.onlyMoveGap }}cp</span>
            <span
              v-if="r.missRate != null"
              class="text-neutral-400"
            >miss {{ Math.round(r.missRate * 100) }}%</span>
            <span
              v-if="r.trap"
              class="text-neutral-400"
            >{{ r.trap }}</span>
            <span
              v-if="r.attack"
              class="text-neutral-400"
            >storm {{ r.attack.path.join('→') }}</span>
          </span>
        </li>
      </ul>
    </div>
  </section>
</template>
