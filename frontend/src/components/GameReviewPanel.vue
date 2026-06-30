<script setup lang="ts">
// Engine game-review panel (issue #136, extends #119): the analyze/export
// controls, the eval graph, the per-side accuracy summary and the why-note for
// the board's current move. Split out of GamesView so the view stays under the
// file cap. Drives the games/review stores directly: analysing grafts the
// engine's better lines onto the board tree (#136), and the eval graph navigates
// the board by mapping a clicked ply back to its mainline node.
import EvalGraph from './EvalGraph.vue'
import SaveAsAnalysisForm from './SaveAsAnalysisForm.vue'
import { useGamesStore } from '../stores/games'
import { useReviewStore } from '../stores/review'
import { downloadText } from '../lib/download'
import { classificationClass, classificationGlyph, formatReviewEval } from '../lib/reviewFormat'

// Engine capability flag from `/api/health` (owned by the parent view); null
// until fetched, false disables the engine-backed actions.
defineProps<{ engineEnabled: boolean | null }>()

const games = useGamesStore()
const review = useReviewStore()

/** White accuracy etc. formatted as "xx.x%". */
function pct(n: number): string {
  return `${n.toFixed(1)}%`
}

async function onAnalyse() {
  if (!games.openGame) return
  const result = await review.analyse(games.openGame.id)
  // Graft the engine's better lines at critical positions onto the live tree.
  if (result) games.graftReview(result)
}

// Export the open game as a `.pgn` download (issue #120): verbatim, or — with
// `annotated` — carrying the engine analysis (`[%eval]` + NAGs + why-notes).
async function onExport(annotated: boolean) {
  const game = games.openGame
  if (!game) return
  const pgn = await games.exportPgn(annotated)
  if (pgn != null) downloadText(`game-${game.id}.pgn`, pgn)
}

// Clicking the eval curve jumps the board to that ply's mainline node.
function onGraphSelect(ply: number) {
  const id = games.nodeAtPly(ply)
  if (id != null) games.goto(id)
}
</script>

<template>
  <div>
    <div class="relative flex flex-wrap items-center gap-2">
      <button
        type="button"
        data-test="analyse"
        class="rounded bg-neutral-800 px-3 py-1 text-sm text-white hover:bg-neutral-700 disabled:opacity-50"
        :disabled="review.loading || engineEnabled === false"
        :title="engineEnabled === false ? 'No engine configured on the server.' : ''"
        @click="onAnalyse"
      >
        {{ review.loading ? 'Analyzing…' : 'Analyze game' }}
      </button>
      <button
        type="button"
        data-test="export"
        class="rounded border border-neutral-300 px-3 py-1 text-sm hover:bg-neutral-100"
        @click="onExport(false)"
      >
        Export PGN
      </button>
      <button
        type="button"
        data-test="export-annotated"
        class="rounded border border-neutral-300 px-3 py-1 text-sm hover:bg-neutral-100 disabled:opacity-50"
        :disabled="engineEnabled === false"
        :title="engineEnabled === false ? 'No engine configured on the server.' : ''"
        @click="onExport(true)"
      >
        Export with analysis
      </button>
      <SaveAsAnalysisForm :engine-enabled="engineEnabled" />
      <span
        v-if="engineEnabled === false"
        class="text-xs text-neutral-500"
      >
        No engine configured.
      </span>
      <span
        v-if="review.error"
        class="text-xs text-red-600"
        data-test="review-error"
      >
        {{ review.error }}
      </span>
    </div>

    <!-- Engine review: graph, accuracy summary, and the current-move note. -->
    <div
      v-if="review.review"
      class="mt-4"
      data-test="review-panel"
    >
      <EvalGraph
        :moves="review.review.moves"
        :current-ply="games.plyOf(games.currentId) ?? 0"
        @select="onGraphSelect"
      />

      <div class="mt-3 grid grid-cols-2 gap-3 text-xs">
        <div
          v-for="side in (['white', 'black'] as const)"
          :key="side"
          class="rounded border border-neutral-200 p-2"
          :data-test="`summary-${side}`"
        >
          <p class="mb-1 font-medium capitalize">
            {{ side }}
          </p>
          <p>Accuracy: {{ pct(review.review.summary[side].accuracy) }}</p>
          <p>ACPL: {{ review.review.summary[side].acpl }}</p>
          <p class="text-neutral-500">
            {{ review.review.summary[side].inaccuracies }} inacc ·
            {{ review.review.summary[side].mistakes }} mist ·
            {{ review.review.summary[side].blunders }} blun
          </p>
        </div>
      </div>

      <div
        v-if="review.currentMove"
        class="mt-3 rounded border border-neutral-200 p-2 text-sm"
        data-test="why-note"
      >
        <span
          class="font-medium"
          :class="classificationClass(review.currentMove.classification)"
        >
          {{ review.currentMove.san }}{{ classificationGlyph(review.currentMove.classification) }}
        </span>
        <span class="text-neutral-500"> {{ formatReviewEval(review.currentMove) }}</span>
        <span
          v-if="review.currentMove.best_move"
          class="text-neutral-500"
        > · best: {{ review.currentMove.best_move }}</span>
        <p class="mt-1 text-neutral-700">
          {{ review.currentMove.explanation }}
        </p>
      </div>
    </div>

    <!-- Analyses saved from this game (issue #164). -->
    <div
      v-if="games.linkedStudies.length"
      class="mt-4"
      data-test="linked-analyses"
    >
      <p class="mb-1 text-xs font-medium text-neutral-500">
        Saved analyses
      </p>
      <ul class="flex flex-col gap-0.5 text-sm">
        <li
          v-for="s in games.linkedStudies"
          :key="s.id"
        >
          <RouterLink
            :to="{ name: 'studies' }"
            data-test="linked-analysis"
            class="text-neutral-700 hover:underline"
          >
            {{ s.name }}
          </RouterLink>
        </li>
      </ul>
    </div>
  </div>
</template>
