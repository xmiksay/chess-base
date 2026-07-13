<script setup lang="ts">
// LLM study-generation dialog (issue #119, Mode B). A focused modal: pick a
// target database, name, start FEN, engine depth, and the two repertoire-framing
// knobs (variation depth = max_depth, breadth = max_children). On submit it calls
// the studies store, which generates + verifies + persists, then refreshes the
// list. The result panel reports committed nodes vs rejected (verification-
// dropped) claims and offers to open the new study.
import { computed, onMounted, ref } from 'vue'
import { api } from '../api'
import { useStudiesStore } from '../stores/studies'
import { STARTPOS_FEN } from '../lib/fen'
import type { Database, GenerateBody, GenerateView } from '../types'

interface Props {
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
const startFen = ref(props.startFen || STARTPOS_FEN)
const engineDepth = ref(18)
const maxDepth = ref(6)
const maxChildren = ref(3)
// Board-arrow annotation: pin engine "plan" trajectories (0–3 lines) and/or the
// static hanging-piece "threats" onto every node (#123/#60). Off by default.
const planLines = ref(0)
const threats = ref(false)
// Player/color/date filter narrowing which games the tree-builder draws
// continuations from (issue #172); blank ⇒ unfiltered.
const player = ref('')
const color = ref('')
const dateFrom = ref('')
const dateTo = ref('')

const COLORS = [
  { value: '', label: 'Either side' },
  { value: 'white', label: 'as White' },
  { value: 'black', label: 'as Black' },
]

const running = ref(false)
const error = ref<string | null>(null)
const result = ref<GenerateView | null>(null)

const canSubmit = computed(
  () => props.llmEnabled && !running.value && !!name.value.trim() && databaseId.value != null,
)

async function onSubmit() {
  if (!canSubmit.value || databaseId.value == null) return
  running.value = true
  error.value = null
  result.value = null
  const body: GenerateBody = {
    database_id: databaseId.value,
    name: name.value.trim(),
    start_fen: startFen.value.trim() || STARTPOS_FEN,
    engine_depth: engineDepth.value,
    tree: { max_depth: maxDepth.value, max_children: maxChildren.value },
    plan_lines: planLines.value,
    threats: threats.value,
  }
  const trimmedPlayer = player.value.trim()
  if (trimmedPlayer) body.player = trimmedPlayer
  if (color.value) body.color = color.value
  const trimmedFrom = dateFrom.value.trim()
  if (trimmedFrom) body.date_from = trimmedFrom
  const trimmedTo = dateTo.value.trim()
  if (trimmedTo) body.date_to = trimmedTo
  try {
    result.value = await studies.generate(body)
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
    data-test="generate-dialog"
    @click.self="emit('close')"
  >
    <div class="w-full max-w-md rounded bg-surface p-5 shadow-lg">
      <header class="mb-4 flex items-center justify-between">
        <h3 class="text-base font-semibold">
          Generate study
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
            placeholder="e.g. Najdorf for Black"
            class="rounded border border-border px-2 py-1"
          >
        </label>

        <label class="flex flex-col gap-1 text-sm">
          Start position (FEN)
          <input
            v-model="startFen"
            data-test="start-fen"
            class="rounded border border-border px-2 py-1 font-mono text-xs"
          >
        </label>

        <div class="grid grid-cols-3 gap-2 text-sm">
          <label class="flex flex-col gap-1">
            Engine depth
            <input
              v-model.number="engineDepth"
              data-test="engine-depth"
              type="number"
              min="1"
              class="rounded border border-border px-2 py-1"
            >
          </label>
          <label class="flex flex-col gap-1">
            Variation depth
            <input
              v-model.number="maxDepth"
              data-test="max-depth"
              type="number"
              min="1"
              class="rounded border border-border px-2 py-1"
            >
          </label>
          <label class="flex flex-col gap-1">
            Breadth
            <input
              v-model.number="maxChildren"
              data-test="max-children"
              type="number"
              min="1"
              class="rounded border border-border px-2 py-1"
            >
          </label>
        </div>

        <!-- Restrict continuations to one player's games (issue #172). -->
        <div class="grid grid-cols-2 gap-2 text-sm">
          <label class="flex flex-col gap-1">
            Player
            <input
              v-model="player"
              data-test="player"
              placeholder="e.g. Carlsen"
              class="rounded border border-border px-2 py-1"
            >
          </label>
          <label class="flex flex-col gap-1">
            Color
            <select
              v-model="color"
              data-test="color"
              class="rounded border border-border px-2 py-1"
            >
              <option
                v-for="c in COLORS"
                :key="c.value"
                :value="c.value"
              >
                {{ c.label }}
              </option>
            </select>
          </label>
          <label class="flex flex-col gap-1">
            Date from
            <input
              v-model="dateFrom"
              data-test="date-from"
              placeholder="YYYY.MM.DD"
              class="rounded border border-border px-2 py-1"
            >
          </label>
          <label class="flex flex-col gap-1">
            Date to
            <input
              v-model="dateTo"
              data-test="date-to"
              placeholder="YYYY.MM.DD"
              class="rounded border border-border px-2 py-1"
            >
          </label>
        </div>

        <!-- Board-arrow annotations baked onto every node. -->
        <div class="flex items-end gap-4 text-sm">
          <label class="flex flex-col gap-1">
            Plan lines
            <input
              v-model.number="planLines"
              data-test="plan-lines"
              type="number"
              min="0"
              max="3"
              class="w-20 rounded border border-border px-2 py-1"
            >
          </label>
          <label class="flex items-center gap-2 pb-1.5">
            <input
              v-model="threats"
              data-test="threats"
              type="checkbox"
              class="rounded border-border"
            >
            Threat arrows
          </label>
        </div>

        <p
          v-if="!llmEnabled"
          class="text-xs text-muted"
          data-test="llm-hint"
        >
          Set ANTHROPIC_API_KEY to enable AI study generation.
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
