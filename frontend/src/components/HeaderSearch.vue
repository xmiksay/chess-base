<script setup lang="ts">
// Header/metadata search (issue #6): a form over player/color/event/result/ECO/
// date that renders the matching games. Query state + param mapping live in the
// store and lib/headerQuery; results are keyset-paginated ("Load more").
import { useSearchStore } from '../stores/search'

const search = useSearchStore()

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
  search.runHeaderSearch()
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
          @click="search.resetQuery()"
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
      v-if="search.searched && !search.headerLoading && search.results.length === 0"
      class="text-sm text-muted"
    >
      No games match.
    </p>

    <div
      v-if="search.results.length"
      class="overflow-x-auto"
    >
      <table class="w-full border-collapse text-sm">
        <thead>
          <tr class="border-b border-border text-left text-muted">
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
  </div>
</template>
