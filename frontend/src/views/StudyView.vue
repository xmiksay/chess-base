<script setup lang="ts">
// Variation-tree editor (issue #8): pick/create a study, play moves on the board
// to build a mainline + variations, navigate the tree, and annotate moves. The
// board (chessground) and tree stay in sync through the study-editor store.
import { computed, onMounted, onUnmounted, ref } from 'vue'
import Board from '../components/Board.vue'
import MoveTree from '../components/MoveTree.vue'
import AnnotationEditor from '../components/AnnotationEditor.vue'
import StudyAnalysis from '../components/StudyAnalysis.vue'
import GenerateStudyDialog from '../components/GenerateStudyDialog.vue'
import { api } from '../api'
import { downloadText } from '../lib/download'
import { useStudiesStore } from '../stores/studies'
import { useStudyEditorStore } from '../stores/studyEditor'
import { useSettingsStore } from '../stores/settings'
import { useEngineStore } from '../stores/engine'
import type { DrawShape } from 'chessground/draw'
import type { BoardMove, Database, Shape } from '../types'

const studies = useStudiesStore()
const editor = useStudyEditorStore()
const settings = useSettingsStore()
const engine = useEngineStore()

// Live engine PV arrows, overlaid on the board without touching pinned plans.
const engineShapes = computed(() => engine.shapes as unknown as DrawShape[])

const databases = ref<Database[]>([])
const newName = ref('')
const newDb = ref<number | null>(null)
const loadError = ref<string | null>(null)
// LLM capability flag from `/api/health`, and the generate-dialog toggle.
const llmEnabled = ref(false)
const showGenerate = ref(false)

async function onGenerated(id: number) {
  showGenerate.value = false
  await editor.open(id)
}

const hasStudy = computed(() => !!studies.current)

// Export the open study as a `.pgn` download (issue #120). `withEval` keeps the
// per-move `[%eval]` annotations (extended); `false` exports plain movetext.
async function onExport(withEval: boolean) {
  const study = studies.current
  if (!study) return
  try {
    const pgn = await studies.exportPgn(study.id, withEval)
    downloadText(`study-${study.id}.pgn`, pgn)
  } catch (e) {
    loadError.value = String((e as Error)?.message ?? e)
  }
}

// Pinned plan arrows on the selected node (#61); the stored `Shape` model
// matches chessground's `DrawShape` (orig/dest/brush).
const pinnedShapes = computed(
  () => (editor.currentNode?.shapes ?? []) as unknown as DrawShape[],
)

async function onBoardMove({ from, to }: BoardMove) {
  try {
    await editor.playMove({ from, to })
  } catch (e) {
    loadError.value = String((e as Error)?.message ?? e)
  }
}

// Persist the arrows/highlights the user drew on the board as the node's pinned
// plan (#61). Normalise to the stored `Shape` shape so no transient chessground
// fields leak into `tree_json`.
async function onShapesDrawn(shapes: Shape[]) {
  try {
    await editor.setShapes(
      shapes.map((s) => ({ orig: s.orig, dest: s.dest ?? null, brush: s.brush || 'green' })),
    )
  } catch (e) {
    loadError.value = String((e as Error)?.message ?? e)
  }
}

async function onCreate() {
  if (!newName.value.trim() || newDb.value == null) return
  await studies.create(newDb.value, newName.value.trim())
  editor.select(studies.current?.tree?.root ?? 0)
  newName.value = ''
  await studies.refresh()
}

function onKey(e: KeyboardEvent) {
  if (!hasStudy.value) return
  const target = e.target as HTMLElement | null
  if (target && (target.tagName === 'TEXTAREA' || target.tagName === 'INPUT')) return
  if (e.key === 'ArrowLeft') {
    editor.back()
    e.preventDefault()
  } else if (e.key === 'ArrowRight') {
    editor.forward()
    e.preventDefault()
  } else if (e.key === 'ArrowUp' || e.key === 'Home') {
    editor.goToStart()
    e.preventDefault()
  } else if (e.key === 'ArrowDown' || e.key === 'End') {
    editor.goToEnd()
    e.preventDefault()
  }
}

onMounted(async () => {
  window.addEventListener('keydown', onKey)
  api.health().then((h) => (llmEnabled.value = h.llm === true)).catch(() => {})
  try {
    await studies.refresh()
    databases.value = await api.databases.list()
    newDb.value =
      databases.value.find((d) => d.id === settings.defaultDatabaseId)?.id ??
      databases.value[0]?.id ??
      null
  } catch (e) {
    loadError.value = String((e as Error)?.message ?? e)
  }
})

onUnmounted(() => window.removeEventListener('keydown', onKey))
</script>

<template>
  <div class="mx-auto max-w-6xl p-6">
    <header class="mb-4 flex items-center gap-3">
      <h2 class="text-lg font-semibold">
        Studies
      </h2>
      <button
        v-if="hasStudy"
        type="button"
        data-test="export"
        class="ml-auto rounded border border-neutral-300 px-3 py-1 text-sm hover:bg-neutral-100"
        @click="onExport(false)"
      >
        Export PGN
      </button>
      <button
        v-if="hasStudy"
        type="button"
        data-test="export-eval"
        class="rounded border border-neutral-300 px-3 py-1 text-sm hover:bg-neutral-100"
        @click="onExport(true)"
      >
        Export with eval
      </button>
      <button
        type="button"
        data-test="open-generate"
        :class="['rounded border border-neutral-300 px-3 py-1 text-sm hover:bg-neutral-100', { 'ml-auto': !hasStudy }]"
        @click="showGenerate = true"
      >
        Generate study
      </button>
    </header>

    <GenerateStudyDialog
      v-if="showGenerate"
      :llm-enabled="llmEnabled"
      @close="showGenerate = false"
      @open="onGenerated"
    />

    <p
      v-if="loadError || studies.error"
      class="mb-3 text-sm text-red-600"
      data-test="error"
    >
      {{ loadError || studies.error }}
    </p>

    <div class="flex flex-col gap-6 lg:flex-row">
      <!-- Study list + create -->
      <section class="lg:w-1/4">
        <ul class="mb-4 flex flex-col gap-1">
          <li
            v-for="s in studies.list"
            :key="s.id"
          >
            <button
              type="button"
              data-test="study-row"
              class="w-full rounded px-2 py-1 text-left text-sm hover:bg-neutral-100"
              :class="{ 'bg-neutral-100 font-medium': studies.current?.id === s.id }"
              @click="editor.open(s.id)"
            >
              {{ s.name }}{{ s.global ? ' (global)' : '' }}
            </button>
          </li>
        </ul>

        <form
          data-test="create-form"
          class="flex flex-col gap-2"
          @submit.prevent="onCreate"
        >
          <input
            v-model="newName"
            placeholder="New study name"
            class="rounded border border-neutral-300 px-2 py-1 text-sm"
          >
          <select
            v-model="newDb"
            aria-label="Database"
            class="rounded border border-neutral-300 px-2 py-1 text-sm"
          >
            <option
              v-for="d in databases"
              :key="d.id"
              :value="d.id"
            >
              {{ d.name }}{{ d.global ? ' (global)' : '' }}
            </option>
          </select>
          <button
            type="submit"
            class="rounded bg-neutral-800 px-3 py-1 text-sm text-white hover:bg-neutral-700 disabled:opacity-50"
            :disabled="!newName.trim() || newDb == null"
          >
            Create study
          </button>
        </form>
      </section>

      <!-- Board -->
      <section
        v-if="hasStudy"
        class="lg:w-2/5"
      >
        <Board
          :fen="editor.fen"
          :dests="editor.legalDests"
          :movable="true"
          :last-move="editor.lastMove"
          :board-theme="settings.boardTheme"
          :shapes="pinnedShapes"
          :overlay-shapes="engineShapes"
          :persist-shapes="true"
          :editable-shapes="true"
          @move="onBoardMove"
          @drawn="onShapesDrawn"
        />

        <div class="mt-3 flex items-center gap-2">
          <button
            class="rounded border border-neutral-300 px-2 py-1 text-sm disabled:opacity-50"
            :disabled="editor.atStart"
            aria-label="Start"
            @click="editor.goToStart()"
          >
            ⏮
          </button>
          <button
            class="rounded border border-neutral-300 px-2 py-1 text-sm disabled:opacity-50"
            :disabled="editor.atStart"
            aria-label="Back"
            @click="editor.back()"
          >
            ◀
          </button>
          <button
            class="rounded border border-neutral-300 px-2 py-1 text-sm disabled:opacity-50"
            :disabled="editor.atEnd"
            aria-label="Forward"
            @click="editor.forward()"
          >
            ▶
          </button>
          <button
            class="rounded border border-neutral-300 px-2 py-1 text-sm disabled:opacity-50"
            :disabled="editor.atEnd"
            aria-label="End"
            @click="editor.goToEnd()"
          >
            ⏭
          </button>
          <button
            v-if="editor.currentNode?.shapes?.length"
            class="ml-auto rounded border border-neutral-300 px-2 py-1 text-sm"
            data-test="clear-pin"
            @click="editor.setShapes([])"
          >
            Clear pinned plan
          </button>
        </div>

        <!-- Engine analysis for the selected node (#5 in studies). -->
        <StudyAnalysis class="mt-4" />
      </section>

      <!-- Tree + annotations -->
      <section
        v-if="hasStudy"
        class="lg:w-1/3"
      >
        <p class="mb-2 text-sm font-medium">
          {{ studies.current?.name }}
        </p>
        <MoveTree
          :tree="editor.tree"
          :current-id="editor.nodeId"
          @select="editor.select($event)"
        />
        <hr class="my-3 border-neutral-200">
        <AnnotationEditor
          :node="editor.currentNode"
          @comment="editor.annotate({ comment: $event })"
          @nag="editor.annotate({ nag: $event })"
          @promote="editor.promote(editor.nodeId)"
          @delete="editor.deleteNode(editor.nodeId)"
        />
      </section>

      <p
        v-else
        class="text-sm text-neutral-500"
      >
        Select or create a study to start editing.
      </p>
    </div>
  </div>
</template>
