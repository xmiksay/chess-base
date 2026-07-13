<script setup lang="ts">
// Header/metadata search (issue #6): a form over player/color/event/result/ECO/
// date that renders the matching games. Query state + param mapping live in the
// store and lib/headerQuery; results are keyset-paginated ("Load more").
//
// Issue #171: results carry a multi-select (+ select-all over what's currently
// loaded) driving a bulk-action toolbar — merge into a study, export the
// selection as one `.pgn`, or delete. Selection is local to this component
// (mirrors GamesView's merge selection, issue #170); ids stay valid across
// "Load more" pages since the store only ever appends.
import { computed, ref } from 'vue'
import { useRouter } from 'vue-router'
import { api } from '../api'
import { useSearchStore } from '../stores/search'
import { downloadText } from '../lib/download'
import MergeGamesDialog from './MergeGamesDialog.vue'

const search = useSearchStore()
const router = useRouter()

const RESULTS = [
  { value: '', label: 'Any result' },
  { value: '1-0', label: 'White wins (1-0)' },
  { value: '0-1', label: 'Black wins (0-1)' },
  { value: '1/2-1/2', label: 'Draw (½-½)' },
  { value: '*', label: 'Unfinished (*)' },
]

const COLORS = [
  { value: '', label: 'Either side' },
  { value: 'white', label: 'as White' },
  { value: 'black', label: 'as Black' },
]

function submit() {
  clearSelection()
  search.runHeaderSearch()
}

function clearQuery() {
  clearSelection()
  search.resetQuery()
}

// --- Multi-select + bulk actions (issue #171) -------------------------------

const selected = ref<Set<number>>(new Set())
const allSelected = computed(
  () => search.results.length > 0 && search.results.every((g) => selected.value.has(g.id)),
)

const showMergeDialog = ref(false)
const exporting = ref(false)
const deleting = ref(false)
const bulkError = ref<string | null>(null)

/** Drop the selection and any stale bulk-action error (a fresh search or query
 * reset can make selected ids disappear from the table entirely). */
function clearSelection() {
  selected.value = new Set()
  bulkError.value = null
}

function toggleSelected(id: number) {
  const next = new Set(selected.value)
  if (!next.delete(id)) next.add(id)
  selected.value = next
  bulkError.value = null
}

function toggleSelectAll() {
  selected.value = allSelected.value ? new Set() : new Set(search.results.map((g) => g.id))
  bulkError.value = null
}

function onMerged() {
  showMergeDialog.value = false
  clearSelection()
  router.push({ name: 'studies' })
}

async function exportSelected() {
  const ids = [...selected.value]
  if (!ids.length) return
  exporting.value = true
  bulkError.value = null
  try {
    const pgn = await api.games.exportSelected(ids)
    downloadText('games-export.pgn', pgn)
  } catch (e) {
    bulkError.value = String((e as Error)?.message ?? e)
  } finally {
    exporting.value = false
  }
}

async function deleteSelected() {
  const ids = [...selected.value]
  if (!ids.length) return
  if (!window.confirm(`Delete ${ids.length} game${ids.length === 1 ? '' : 's'}? This cannot be undone.`)) {
    return
  }
  deleting.value = true
  bulkError.value = null
  const failed = new Set<number>()
  for (const id of ids) {
    try {
      await api.games.remove(id)
    } catch {
      failed.add(id)
    }
  }
  search.removeResults(new Set(ids.filter((id) => !failed.has(id))))
  selected.value = failed
  if (failed.size) {
    bulkError.value = `Failed to delete ${failed.size} game${failed.size === 1 ? '' : 's'} (not yours, or already gone).`
  }
  deleting.value = false
}
</script>

<template>
  <div class="flex flex-col gap-4">
    <form
      class="grid grid-cols-1 gap-3 sm:grid-cols-2 lg:grid-cols-3"
      @submit.prevent="submit"
    >
      <label class="flex flex-col gap-1 text-sm">
        <span class="text-muted">Player</span>
        <input
          v-model="search.query.player"
          type="text"
          placeholder="e.g. Carlsen"
          class="rounded border border-border px-2 py-1 bg-surface"
        >
      </label>
      <label class="flex flex-col gap-1 text-sm">
        <span class="text-muted">Color</span>
        <select
          v-model="search.query.color"
          class="rounded border border-border px-2 py-1 bg-surface"
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
      <label class="flex flex-col gap-1 text-sm">
        <span class="text-muted">Event</span>
        <input
          v-model="search.query.event"
          type="text"
          placeholder="e.g. Candidates"
          class="rounded border border-border px-2 py-1 bg-surface"
        >
      </label>
      <label class="flex flex-col gap-1 text-sm">
        <span class="text-muted">Result</span>
        <select
          v-model="search.query.result"
          class="rounded border border-border px-2 py-1 bg-surface"
        >
          <option
            v-for="r in RESULTS"
            :key="r.value"
            :value="r.value"
          >
            {{ r.label }}
          </option>
        </select>
      </label>
      <label class="flex flex-col gap-1 text-sm">
        <span class="text-muted">ECO (prefix)</span>
        <input
          v-model="search.query.eco"
          type="text"
          placeholder="e.g. B9"
          class="rounded border border-border px-2 py-1 bg-surface"
        >
      </label>
      <label class="flex flex-col gap-1 text-sm">
        <span class="text-muted">Date from</span>
        <input
          v-model="search.query.dateFrom"
          type="text"
          placeholder="YYYY.MM.DD"
          class="rounded border border-border px-2 py-1 bg-surface"
        >
      </label>
      <label class="flex flex-col gap-1 text-sm">
        <span class="text-muted">Date to</span>
        <input
          v-model="search.query.dateTo"
          type="text"
          placeholder="YYYY.MM.DD"
          class="rounded border border-border px-2 py-1 bg-surface"
        >
      </label>
      <div class="flex items-end gap-2">
        <button
          type="submit"
          :disabled="search.headerLoading"
          class="rounded bg-accent px-3 py-1 text-sm font-medium text-surface hover:opacity-90 disabled:opacity-50"
        >
          {{ search.headerLoading ? 'Searching…' : 'Search' }}
        </button>
        <button
          type="button"
          class="rounded border border-border px-3 py-1 text-sm"
          @click="clearQuery"
        >
          Clear
        </button>
      </div>
    </form>

    <p
      v-if="search.headerError"
      class="text-sm text-bad"
    >
      {{ search.headerError }}
    </p>

    <p
      v-if="search.searched && !search.headerLoading && search.results.length === 0 && !search.hasMore"
      class="text-sm text-muted"
    >
      No games match.
    </p>

    <!-- Bulk-action toolbar (issue #171): merge/export/delete the ticked rows. -->
    <div
      v-if="selected.size"
      class="flex flex-wrap items-center gap-3 rounded border border-border bg-surface-2 px-3 py-2 text-sm"
    >
      <span class="text-muted">{{ selected.size }} selected</span>
      <button
        type="button"
        data-test="merge-selected"
        class="rounded bg-accent px-3 py-1 font-medium text-surface hover:opacity-90 disabled:opacity-50"
        @click="showMergeDialog = true"
      >
        Merge into study…
      </button>
      <button
        type="button"
        data-test="export-selected"
        class="rounded border border-border px-2 py-1 disabled:opacity-50"
        :disabled="exporting"
        @click="exportSelected"
      >
        {{ exporting ? 'Exporting…' : 'Export selected as PGN' }}
      </button>
      <button
        type="button"
        data-test="delete-selected"
        class="rounded border border-border px-2 py-1 text-bad disabled:opacity-50"
        :disabled="deleting"
        @click="deleteSelected"
      >
        {{ deleting ? 'Deleting…' : 'Delete selected' }}
      </button>
      <button
        type="button"
        class="rounded border border-border px-2 py-1"
        @click="clearSelection"
      >
        Clear
      </button>
      <span
        v-if="bulkError"
        class="text-bad"
      >{{ bulkError }}</span>
    </div>

    <div
      v-if="search.results.length || search.hasMore"
      class="overflow-x-auto"
    >
      <table
        v-if="search.results.length"
        class="w-full border-collapse text-sm"
      >
        <thead>
          <tr class="border-b border-border text-left text-muted">
            <th class="py-1 pr-2">
              <input
                type="checkbox"
                data-test="select-all"
                aria-label="Select all loaded results"
                :checked="allSelected"
                @change="toggleSelectAll"
              >
            </th>
            <th class="py-1 pr-3">
              White
            </th>
            <th class="py-1 pr-3">
              Black
            </th>
            <th class="py-1 pr-3">
              Result
            </th>
            <th class="py-1 pr-3">
              Date
            </th>
            <th class="py-1 pr-3">
              ECO
            </th>
          </tr>
        </thead>
        <tbody>
          <tr
            v-for="g in search.results"
            :key="g.id"
            data-test="result-row"
            class="border-b border-border"
          >
            <td class="py-1 pr-2">
              <input
                type="checkbox"
                data-test="select-game"
                :aria-label="`Select ${g.white ?? '?'} – ${g.black ?? '?'}`"
                :checked="selected.has(g.id)"
                @change="toggleSelected(g.id)"
              >
            </td>
            <td class="py-1 pr-3">
              {{ g.white ?? '—' }}<span
                v-if="g.white_elo"
                class="text-muted"
              > ({{ g.white_elo }})</span>
            </td>
            <td class="py-1 pr-3">
              {{ g.black ?? '—' }}<span
                v-if="g.black_elo"
                class="text-muted"
              > ({{ g.black_elo }})</span>
            </td>
            <td class="py-1 pr-3 font-mono">
              {{ g.result ?? '*' }}
            </td>
            <td class="py-1 pr-3">
              {{ g.date ?? '—' }}
            </td>
            <td class="py-1 pr-3 font-mono">
              {{ g.eco ?? '' }}
            </td>
          </tr>
        </tbody>
      </table>

      <button
        v-if="search.hasMore"
        type="button"
        data-test="load-more"
        :disabled="search.headerLoading"
        class="mt-3 rounded border border-border px-3 py-1 text-sm disabled:opacity-50"
        @click="search.loadMore()"
      >
        {{ search.headerLoading ? 'Loading…' : 'Load more' }}
      </button>
    </div>

    <MergeGamesDialog
      v-if="showMergeDialog"
      :game-ids="[...selected]"
      @close="showMergeDialog = false"
      @merged="onMerged"
    />
  </div>
</template>
