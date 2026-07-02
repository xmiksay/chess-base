<script setup lang="ts">
// Danger-map study-generation dialog (issue #131, ADR-0026). Unlike the best-line
// generator, this walks an intended repertoire spine (PGN) and selects for
// *practical difficulty for the opponent* — traps and only-move narrow paths the
// engine adjudicates. A focused modal: target database, name, the repertoire
// spine, which side it plays, walk depth, and the per-variation engine budget. On
// submit it calls the studies store, which generates + verifies + persists. The
// result panel reports committed nodes, rejected claims, and the engine-tagged
// danger roles (Weapon / Caution / Off-book), then offers to open the new study.
import { computed, onMounted, ref } from 'vue'
import { api } from '../api'
import { useStudiesStore } from '../stores/studies'
import { STARTPOS_FEN } from '../lib/fen'
import type { Database, DangerMapBody, DangerMapView } from '../types'

interface Props {
  /** Both an engine and an LLM must be configured for danger-map generation. */
  llmEnabled: boolean
  /** Optional starting FEN (e.g. the current board); defaults to startpos. */
  startFen?: string
}
const props = withDefaults(defineProps<Props>(), { startFen: STARTPOS_FEN })
const emit = defineEmits<{ close: []; open: [id: number] }>()

const studies = useStudiesStore()

const databases = ref<Database[]>([])
const databaseId = ref<number | null>(null)
const name = ref('')
const spinePgn = ref('')
const startFen = ref(props.startFen || STARTPOS_FEN)
const ourSide = ref<'White' | 'Black'>('White')
const maxDepth = ref(8)
const movetimeMs = ref(500)
const multipv = ref(2)

const running = ref(false)
const error = ref<string | null>(null)
const result = ref<DangerMapView | null>(null)

const canSubmit = computed(
  () =>
    props.llmEnabled &&
    !running.value &&
    !!name.value.trim() &&
    !!spinePgn.value.trim() &&
    databaseId.value != null,
)

async function onSubmit() {
  if (!canSubmit.value || databaseId.value == null) return
  running.value = true
  error.value = null
  result.value = null
  const body: DangerMapBody = {
    database_id: databaseId.value,
    name: name.value.trim(),
    spine_pgn: spinePgn.value.trim(),
    start_fen: startFen.value.trim() || STARTPOS_FEN,
    spine: { our_side: ourSide.value, max_depth: maxDepth.value },
    movetime_ms: movetimeMs.value,
    multipv: multipv.value,
  }
  try {
    result.value = await studies.generateDangerMap(body)
  } catch (e) {
    error.value = String((e as Error)?.message ?? e)
  } finally {
    running.value = false
  }
}

function onOpenResult() {
  if (result.value) emit('open', result.value.id)
}

onMounted(async () => {
  try {
    databases.value = await api.databases.list()
    databaseId.value = databases.value[0]?.id ?? null
  } catch (e) {
    error.value = String((e as Error)?.message ?? e)
  }
})
</script>

<template>
  <div
    class="fixed inset-0 z-50 flex items-center justify-center bg-black/40 p-4"
    data-test="danger-dialog"
    @click.self="emit('close')"
  >
    <div class="w-full max-w-md rounded bg-surface p-5 shadow-lg">
      <header class="mb-4 flex items-center justify-between">
        <h3 class="text-base font-semibold">
          Generate danger map
        </h3>
        <button
          type="button"
          class="text-muted hover:text-fg"
          aria-label="Close"
          @click="emit('close')"
        >
          ✕
        </button>
      </header>

      <!-- Result view after a successful run. -->
      <div
        v-if="result"
        data-test="result"
      >
        <p class="text-sm">
          Generated <span class="font-medium">{{ result.name }}</span> —
          {{ result.node_count }} nodes committed, {{ result.rejected }} claims rejected.
        </p>
        <ul
          v-if="result.roles.length"
          data-test="roles"
          class="mt-3 max-h-48 space-y-1 overflow-y-auto text-xs"
        >
          <li
            v-for="r in result.roles"
            :key="r.node_id"
            class="flex items-center justify-between gap-2 rounded bg-surface-2 px-2 py-1"
          >
            <span class="font-mono">{{ r.san ?? '—' }}</span>
            <span class="text-muted">{{ r.kind }} · {{ r.role }}</span>
          </li>
        </ul>
        <div class="mt-4 flex justify-end gap-2">
          <button
            type="button"
            class="rounded border border-border px-3 py-1 text-sm hover:bg-surface-2"
            @click="emit('close')"
          >
            Close
          </button>
          <button
            type="button"
            data-test="open-result"
            class="rounded bg-fg px-3 py-1 text-sm text-surface hover:opacity-90"
            @click="onOpenResult"
          >
            Open study
          </button>
        </div>
      </div>

      <!-- Form. -->
      <form
        v-else
        class="flex flex-col gap-3"
        @submit.prevent="onSubmit"
      >
        <label class="flex flex-col gap-1 text-sm">
          Target database
          <select
            v-model="databaseId"
            data-test="database"
            class="rounded border border-border px-2 py-1"
          >
            <option
              v-for="d in databases"
              :key="d.id"
              :value="d.id"
            >
              {{ d.name }}{{ d.global ? ' (global)' : '' }}
            </option>
          </select>
        </label>

        <label class="flex flex-col gap-1 text-sm">
          Study name
          <input
            v-model="name"
            data-test="name"
            placeholder="e.g. Smith-Morra traps"
            class="rounded border border-border px-2 py-1"
          >
        </label>

        <label class="flex flex-col gap-1 text-sm">
          Repertoire spine (PGN)
          <textarea
            v-model="spinePgn"
            data-test="spine-pgn"
            rows="3"
            placeholder="1. e4 c5 2. d4 cxd4 3. c3 *"
            class="rounded border border-border px-2 py-1 font-mono text-xs"
          />
        </label>

        <label class="flex flex-col gap-1 text-sm">
          Start position (FEN)
          <input
            v-model="startFen"
            data-test="start-fen"
            class="rounded border border-border px-2 py-1 font-mono text-xs"
          >
        </label>

        <div class="grid grid-cols-2 gap-2 text-sm">
          <label class="flex flex-col gap-1">
            Our side
            <select
              v-model="ourSide"
              data-test="our-side"
              class="rounded border border-border px-2 py-1"
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
            Walk depth
            <input
              v-model.number="maxDepth"
              data-test="max-depth"
              type="number"
              min="1"
              class="rounded border border-border px-2 py-1"
            >
          </label>
          <label class="flex flex-col gap-1">
            Movetime (ms)
            <input
              v-model.number="movetimeMs"
              data-test="movetime"
              type="number"
              min="1"
              class="rounded border border-border px-2 py-1"
            >
          </label>
          <label class="flex flex-col gap-1">
            MultiPV
            <input
              v-model.number="multipv"
              data-test="multipv"
              type="number"
              min="2"
              class="rounded border border-border px-2 py-1"
            >
          </label>
        </div>

        <p
          v-if="!llmEnabled"
          class="text-xs text-muted"
          data-test="llm-hint"
        >
          Configure an engine and set ANTHROPIC_API_KEY to enable danger-map generation.
        </p>
        <p
          v-if="error"
          class="text-xs text-bad"
          data-test="error"
        >
          {{ error }}
        </p>

        <div class="mt-1 flex justify-end gap-2">
          <button
            type="button"
            class="rounded border border-border px-3 py-1 text-sm hover:bg-surface-2"
            @click="emit('close')"
          >
            Cancel
          </button>
          <button
            type="submit"
            data-test="submit"
            class="rounded bg-fg px-3 py-1 text-sm text-surface hover:opacity-90 disabled:opacity-50"
            :disabled="!canSubmit"
          >
            {{ running ? 'Generating…' : 'Generate' }}
          </button>
        </div>
      </form>
    </div>
  </div>
</template>
