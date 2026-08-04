<script setup lang="ts">
// Variation-tree editor (issue #8): pick/create a study, play moves on the board
// to build a mainline + variations, navigate the tree, and annotate moves. The
// board (chessground) and tree stay in sync through the study-editor store.
import { computed, onMounted, onUnmounted, ref, watch } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import Board from '../components/Board.vue'
import BoardControls from '../components/BoardControls.vue'
import BoardEvalBar from '../components/BoardEvalBar.vue'
import MoveTree from '../components/MoveTree.vue'
import MoveComment from '../components/MoveComment.vue'
import AnnotationEditor from '../components/AnnotationEditor.vue'
import StudyAnalysis from '../components/StudyAnalysis.vue'
import GenerateStudyDialog from '../components/GenerateStudyDialog.vue'
import GenerateDangerMapDialog from '../components/GenerateDangerMapDialog.vue'
import DangerMapPanel from '../components/DangerMapPanel.vue'
import StudyFolderSidebar from '../components/StudyFolderSidebar.vue'
import ShareToggle from '../components/ShareToggle.vue'
import { api } from '../api'
import { downloadText } from '../lib/download'
import { useAuthStore } from '../stores/auth'
import { useStudiesStore } from '../stores/studies'
import { useFoldersStore } from '../stores/folders'
import { useStudyEditorStore } from '../stores/studyEditor'
import { useSettingsStore } from '../stores/settings'
import { useDangerStore } from '../stores/danger'
import { useBoardOverlays } from '../lib/useBoardOverlays'
import { numericParam } from '../lib/routeParam'
import { dangerShapesForFen } from '../lib/dangerShapes'
import type { DrawShape } from 'chessground/draw'
import type { BoardMove, Database, Shape } from '../types'

const auth = useAuthStore()
const studies = useStudiesStore()
const folders = useFoldersStore()
const editor = useStudyEditorStore()
const settings = useSettingsStore()
const danger = useDangerStore()
const router = useRouter()
const route = useRoute()

// Anonymous public view (issue #213): a logged-out server-mode visitor on a
// /studies/:id deep link. The sidebar, engine panels and every editing control
// are hidden — the study renders read-only (board, tree, comments, PGN export).
const readOnly = computed(() => auth.isServerMode && !auth.user)

// Toggleable overlay layers (plans / threats / master, #123) driven by the
// selected node's FEN. The engine-PV arrows ride along as the Plans layer, so
// they stay read-only auto-shapes that never clobber the node's pinned drawings.
const { boardShapes, clear: clearEngineOverlays } = useBoardOverlays(() => editor.fen)

// Engine-only danger arrows (#156): the dangerous replies available from the
// selected node, derived locally from the walked DangerTree. Composed on top of
// the standard overlay layers so they coexist with plans / threats / master.
// "Clear arrows" (issue #190) hides this layer without discarding the walked
// tree (the side panel keeps its data); re-running the walk shows it again.
const dangerVisible = ref(true)
watch(() => danger.tree, () => { dangerVisible.value = true })
const overlayShapes = computed(() => [
  ...boardShapes.value,
  ...(dangerVisible.value ? dangerShapesForFen(danger.tree, editor.fen) : []),
])

/** "Clear arrows" (issue #190): turn off the live engine-analysis overlays
 * (plans/threats/master/danger) and keep them off. Never touches the node's
 * persisted pinned shapes — those have their own "Clear pinned plan" control. */
function clearArrows() {
  clearEngineOverlays()
  dangerVisible.value = false
}

/** Delete a node (and its subtree) from the open study, after confirming. */
async function onRemoveNode(id: number) {
  if (!window.confirm('Delete this move and everything after it in the line?')) return
  try {
    await editor.deleteNode(id)
  } catch (e) {
    loadError.value = String((e as Error)?.message ?? e)
  }
}

const databases = ref<Database[]>([])
const loadError = ref<string | null>(null)

// Open a study in the editor (delegated from the folder sidebar, #164). Selecting
// the root node mirrors the create flow so the board starts at the start position.
async function onOpenStudy(id: number) {
  try {
    await editor.open(id)
  } catch (e) {
    loadError.value = String((e as Error)?.message ?? e)
  }
}

// --- URL addressability (issue #212): /studies/:id? + ?folder= ---------------

// The folder selected in the sidebar (null ⇒ root / "Unfiled"), owned here so
// it can be hydrated from and mirrored to the URL; the sidebar takes it as a
// v-model prop. Seeded straight from the query so the first render already
// shows the linked folder.
const selectedFolderId = ref<number | null>(numericParam(route.query.folder))

// Selection → URL: mirror the open study and folder. `replace` keeps in-page
// selection out of the history stack; the guard stops hydration echoes.
watch(
  () => [studies.current?.id, selectedFolderId.value] as const,
  ([id, folderId]) => {
    if (route.name !== 'studies') return
    if (numericParam(route.params.id) === (id ?? null) && numericParam(route.query.folder) === folderId) return
    const query = { ...route.query }
    if (folderId != null) query.folder = String(folderId)
    else delete query.folder
    void router.replace({ name: 'studies', params: id != null ? { id: String(id) } : {}, query })
  },
)

// URL → selection: hydrate on back/forward (mount is handled in onMounted).
watch(
  () => [route.params.id, route.query.folder] as const,
  async () => {
    if (route.name !== 'studies') return
    selectedFolderId.value = numericParam(route.query.folder)
    const id = numericParam(route.params.id)
    if (id != null && id !== studies.current?.id) await onOpenStudy(id)
  },
)
// Capability flags from `/api/health`: LLM gates the generate dialogs; the engine
// alone gates the engine-only danger overlay (#156). Plus the generate-dialog toggle.
const llmEnabled = ref(false)
const engineEnabled = ref(false)
const showGenerate = ref(false)
const showDangerMap = ref(false)

async function onGenerated(id: number) {
  showGenerate.value = false
  showDangerMap.value = false
  await editor.open(id)
}

const hasStudy = computed(() => !!studies.current)

// --- sharing (issue #213) ----------------------------------------------------

// Whether the caller may toggle sharing: local mode always; server mode needs a
// login and either admin or a non-global study. The server enforces regardless
// — this only hides a guaranteed 403.
const canShare = computed(() => {
  if (!auth.isServerMode) return true
  if (!auth.user) return false
  return auth.user.is_admin || !studies.current?.global
})

// The open study's shareable deep link.
const shareUrl = computed(() =>
  studies.current
    ? window.location.origin +
      router.resolve({ name: 'studies', params: { id: String(studies.current.id) } }).href
    : '',
)

async function onTogglePublic() {
  const study = studies.current
  if (!study) return
  try {
    await studies.setPublic(study.id, !study.public)
  } catch {
    // Already surfaced via studies.error (rendered above the board).
  }
}

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
// matches chessground's `DrawShape` (orig/dest/brush). Copy the array — Board
// hands it straight to chessground, which mutates its shapes layer in place
// (issue #190), and that must never happen to the Pinia-held original.
const pinnedShapes = computed(
  () => [...(editor.currentNode?.shapes ?? [])] as unknown as DrawShape[],
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
  api
    .health()
    .then((h) => {
      llmEnabled.value = h.llm === true
      engineEnabled.value = h.engine === true
    })
    .catch(() => {})
  // Anonymous public view (issue #213): hydrate only the deep-linked study —
  // the studies/folders/databases refreshes would all 401.
  if (readOnly.value) {
    const studyId = numericParam(route.params.id)
    if (studyId != null) await onOpenStudy(studyId)
    return
  }
  try {
    await Promise.all([studies.refresh(), folders.refresh()])
    databases.value = await api.databases.list()
    // Deep link straight to a study: /studies/:id (issue #212).
    const studyId = numericParam(route.params.id)
    if (studyId != null && studyId !== studies.current?.id) await onOpenStudy(studyId)
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
      <!-- Share control (issue #213): public toggle + copy the deep link. -->
      <ShareToggle
        v-if="hasStudy"
        class="ml-auto"
        :is-public="studies.current?.public ?? false"
        :can-toggle="canShare"
        :url="shareUrl"
        @toggle="onTogglePublic"
      />
      <button
        v-if="hasStudy"
        type="button"
        data-test="export"
        class="rounded border border-border px-3 py-1 text-sm hover:bg-surface-2"
        @click="onExport(false)"
      >
        Export PGN
      </button>
      <button
        v-if="hasStudy"
        type="button"
        data-test="export-eval"
        class="rounded border border-border px-3 py-1 text-sm hover:bg-surface-2"
        title="Lichess imports the eval gauge + arrows from this PGN; chess.com ignores them. Run 'Analyse study' first to fill the evals."
        @click="onExport(true)"
      >
        Export with eval
      </button>
      <button
        v-if="!readOnly"
        type="button"
        data-test="open-generate"
        :class="['rounded border border-border px-3 py-1 text-sm hover:bg-surface-2', { 'ml-auto': !hasStudy }]"
        @click="showGenerate = true"
      >
        Generate study
      </button>
      <button
        v-if="!readOnly"
        type="button"
        data-test="open-danger-map"
        class="rounded border border-border px-3 py-1 text-sm hover:bg-surface-2"
        @click="showDangerMap = true"
      >
        Danger map
      </button>
    </header>

    <GenerateStudyDialog
      v-if="showGenerate"
      :llm-enabled="llmEnabled"
      @close="showGenerate = false"
      @open="onGenerated"
    />

    <GenerateDangerMapDialog
      v-if="showDangerMap"
      :llm-enabled="llmEnabled"
      @close="showDangerMap = false"
      @open="onGenerated"
    />

    <p
      v-if="loadError || studies.error"
      class="mb-3 text-sm text-bad"
      data-test="error"
    >
      {{ loadError || studies.error }}
    </p>

    <div class="flex flex-col gap-6 lg:flex-row">
      <!-- Folder tree + studies in the selected folder + create (#164) -->
      <StudyFolderSidebar
        v-if="!readOnly"
        v-model:folder-id="selectedFolderId"
        :databases="databases"
        :current-id="studies.current?.id ?? null"
        :default-db-id="settings.defaultDatabaseId ?? null"
        @open="onOpenStudy"
        @error="loadError = $event"
      />

      <!-- Board column: Stockfish on top, then the eval thermometer + board. -->
      <section
        v-if="hasStudy"
        class="flex flex-col gap-4 lg:w-2/5"
      >
        <!-- Engine analysis for the selected node (#5 in studies), above the board. -->
        <div
          v-if="!readOnly"
          class="rounded-lg border border-border bg-surface p-4 shadow-sm"
        >
          <StudyAnalysis />
        </div>

        <div>
          <div class="flex items-stretch gap-2">
            <BoardEvalBar :fen="editor.fen" />
            <Board
              :fen="editor.fen"
              :dests="editor.legalDests"
              :movable="!readOnly"
              :last-move="editor.lastMove"
              :board-theme="settings.boardTheme"
              :shapes="pinnedShapes"
              :overlay-shapes="overlayShapes"
              :persist-shapes="true"
              :editable-shapes="!readOnly"
              @move="onBoardMove"
              @drawn="onShapesDrawn"
            />
          </div>

          <BoardControls
            class="mt-3"
            :at-start="editor.atStart"
            :at-end="editor.atEnd"
            @first="editor.goToStart()"
            @prev="editor.back()"
            @next="editor.forward()"
            @last="editor.goToEnd()"
            @clear-arrows="clearArrows"
          />

          <!-- Read surface for the selected move's comment, right under the board. -->
          <MoveComment
            class="mt-3"
            :tree="editor.tree"
            :current-id="editor.nodeId"
          />

          <!-- Per-view extra: clear the persisted pinned plan on this node (#61). -->
          <button
            v-if="!readOnly && editor.currentNode?.shapes?.length"
            class="mt-2 rounded border border-border px-2 py-1 text-sm hover:bg-surface-2"
            data-test="clear-pin"
            @click="editor.setShapes([])"
          >
            Clear pinned plan
          </button>
        </div>
      </section>

      <!-- Right column: PGN notation on top, danger map below it. -->
      <section
        v-if="hasStudy"
        class="flex flex-1 flex-col gap-4"
      >
        <!-- PGN notation + annotations. -->
        <div class="rounded-lg border border-border bg-surface p-4 shadow-sm">
          <p class="mb-2 flex items-center gap-2 text-sm font-medium">
            <span>{{ studies.current?.name }}</span>
            <RouterLink
              v-if="studies.current?.origin_game_id != null"
              :to="{ name: 'games', params: { id: String(studies.current.origin_game_id) } }"
              data-test="origin-game-link"
              class="rounded bg-surface-2 px-2 py-0.5 text-xs font-normal text-muted hover:text-fg"
            >
              From game #{{ studies.current.origin_game_id }}
            </RouterLink>
          </p>
          <!-- Cap the notation to ~board height and scroll in place; the
               annotation editor below stays visible. -->
          <div class="max-h-[360px] overflow-y-auto">
            <MoveTree
              :tree="editor.tree"
              :current-id="editor.nodeId"
              :editable="!readOnly"
              @select="editor.select($event)"
              @promote="editor.promote($event)"
              @demote="editor.demote($event)"
              @remove="onRemoveNode($event)"
            />
          </div>
          <template v-if="!readOnly">
            <hr class="my-3 border-border">
            <AnnotationEditor
              :node="editor.currentNode"
              @comment="editor.annotate({ comment: $event })"
              @nag="editor.annotate({ nag: $event })"
              @promote="editor.promote(editor.nodeId)"
              @demote="editor.demote(editor.nodeId)"
              @delete="onRemoveNode(editor.nodeId)"
            />
          </template>
        </div>

        <!-- Danger map under the PGN notation: walk a spine for danger and graft
             the lines into the tree (#156). Engine-only, no LLM. -->
        <DangerMapPanel
          v-if="!readOnly"
          :engine-enabled="engineEnabled"
          :study-id="studies.current?.id ?? null"
          :start-fen="editor.tree?.start_fen"
        />
      </section>

      <p
        v-else
        class="text-sm text-muted"
      >
        {{ readOnly ? 'This study is not available.' : 'Select or create a study to start editing.' }}
      </p>
    </div>
  </div>
</template>
