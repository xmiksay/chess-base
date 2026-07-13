<script setup lang="ts">
// Position / opening-tree explorer (issue #7 UI): a board whose position drives
// the move-stats tree. Drag a piece or click a tree row to descend; "back"/"start"
// walk the line. Navigation + stat math are pure (lib/openingTree); this renders.
import { computed, onMounted } from 'vue'
import Board from './Board.vue'
import { useSearchStore } from '../stores/search'
import { useSettingsStore } from '../stores/settings'
import { frequency, scoreBar, totalCount } from '../lib/openingTree'
import { isEmptyFilter } from '../lib/positionFilter'
import type { BoardMove } from '../types'

const search = useSearchStore()
const settings = useSettingsStore()

const total = computed(() => totalCount(search.tree))
const filterIsEmpty = computed(() => isEmptyFilter(search.filter))

const COLORS = [
  { value: '', label: 'Either side' },
  { value: 'white', label: 'as White' },
  { value: 'black', label: 'as Black' },
]

function onMove({ from, to }: BoardMove) {
  search.playMove({ from, to })
}

onMounted(() => {
  if (search.tree.length === 0 && search.games.length === 0) search.loadPosition()
})
</script>

<template>
  <div class="flex flex-col gap-6 md:flex-row">
    <section class="flex flex-col gap-3">
      <Board
        :fen="search.position.fen"
        :dests="search.position.dests"
        :last-move="search.position.lastMove"
        :movable="true"
        :board-theme="settings.boardTheme"
        @move="onMove"
      />
      <div class="flex items-center gap-2">
        <button
          type="button"
          class="rounded border border-border px-3 py-1 text-sm disabled:opacity-50"
          :disabled="search.line.length === 0"
          @click="search.resetBoard()"
        >
          ⏮ Start
        </button>
        <button
          type="button"
          class="rounded border border-border px-3 py-1 text-sm disabled:opacity-50"
          :disabled="search.line.length === 0"
          @click="search.back()"
        >
          ← Back
        </button>
      </div>
      <p class="font-mono text-sm text-muted">
        <span
          v-if="search.line.length === 0"
          class="text-muted"
        >start position</span>
        <span v-else>{{ search.line.join(' ') }}</span>
      </p>
    </section>

    <section class="flex-1">
      <p
        v-if="search.explorerError"
        class="mb-2 text-sm text-bad"
      >
        {{ search.explorerError }}
      </p>

      <form
        data-test="position-filter"
        class="mb-3 flex flex-wrap items-end gap-2"
        @submit.prevent="search.loadPosition()"
      >
        <label class="flex flex-col gap-1 text-xs">
          <span class="text-muted">Player</span>
          <input
            v-model="search.filter.player"
            type="text"
            data-test="filter-player"
            placeholder="e.g. Carlsen"
            class="w-32 rounded border border-border px-2 py-1 bg-surface text-sm"
          >
        </label>
        <label class="flex flex-col gap-1 text-xs">
          <span class="text-muted">Color</span>
          <select
            v-model="search.filter.color"
            data-test="filter-color"
            class="rounded border border-border px-2 py-1 bg-surface text-sm"
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
        <label class="flex flex-col gap-1 text-xs">
          <span class="text-muted">Date from</span>
          <input
            v-model="search.filter.dateFrom"
            type="text"
            data-test="filter-date-from"
            placeholder="YYYY.MM.DD"
            class="w-28 rounded border border-border px-2 py-1 bg-surface text-sm"
          >
        </label>
        <label class="flex flex-col gap-1 text-xs">
          <span class="text-muted">Date to</span>
          <input
            v-model="search.filter.dateTo"
            type="text"
            data-test="filter-date-to"
            placeholder="YYYY.MM.DD"
            class="w-28 rounded border border-border px-2 py-1 bg-surface text-sm"
          >
        </label>
        <button
          type="submit"
          data-test="filter-apply"
          :disabled="search.explorerLoading"
          class="rounded bg-accent px-3 py-1 text-sm font-medium text-surface hover:opacity-90 disabled:opacity-50"
        >
          Apply
        </button>
        <button
          v-if="!filterIsEmpty"
          type="button"
          data-test="filter-clear"
          class="rounded border border-border px-3 py-1 text-sm"
          @click="search.resetFilter()"
        >
          Clear
        </button>
      </form>

      <h3 class="mb-2 text-sm font-semibold text-muted">
        Moves <span v-if="total">({{ total }} games)</span>
      </h3>

      <p
        v-if="!search.explorerLoading && search.tree.length === 0"
        class="text-sm text-muted"
      >
        No games reach this position.
      </p>

      <table
        v-else
        class="w-full border-collapse text-sm"
      >
        <thead>
          <tr class="border-b border-border text-left text-muted">
            <th class="py-1 pr-3">
              Move
            </th>
            <th class="py-1 pr-3">
              Games
            </th>
            <th class="py-1">
              White / Draw / Black
            </th>
          </tr>
        </thead>
        <tbody>
          <tr
            v-for="m in search.tree"
            :key="m.san"
            data-test="tree-row"
            class="cursor-pointer border-b border-border hover:bg-surface-2"
            @click="search.playSan(m.san)"
          >
            <td class="py-1 pr-3 font-mono font-medium">
              {{ m.san }}
            </td>
            <td class="py-1 pr-3">
              {{ m.count }}<span class="text-muted"> ({{ frequency(m, total) }}%)</span>
            </td>
            <td class="py-1">
              <div class="flex h-4 w-40 overflow-hidden rounded text-[10px] leading-4 text-white">
                <div
                  class="bg-neutral-200 text-neutral-700"
                  :style="{ width: scoreBar(m).white + '%' }"
                >
                  <span
                    v-if="scoreBar(m).white >= 12"
                    class="px-1"
                  >{{ scoreBar(m).white }}%</span>
                </div>
                <div
                  class="bg-neutral-400"
                  :style="{ width: scoreBar(m).draws + '%' }"
                />
                <div
                  class="bg-neutral-700"
                  :style="{ width: scoreBar(m).black + '%' }"
                >
                  <span
                    v-if="scoreBar(m).black >= 12"
                    class="px-1"
                  >{{ scoreBar(m).black }}%</span>
                </div>
              </div>
            </td>
          </tr>
        </tbody>
      </table>

      <h3
        v-if="search.games.length"
        class="mb-2 mt-6 text-sm font-semibold text-muted"
      >
        Games
      </h3>
      <ul class="flex flex-col gap-1">
        <li
          v-for="g in search.games"
          :key="g.id"
          data-test="game-row"
          class="text-sm"
        >
          <span class="font-medium">{{ g.white ?? '—' }}</span>
          <span class="text-muted"> vs </span>
          <span class="font-medium">{{ g.black ?? '—' }}</span>
          <span class="font-mono text-muted"> {{ g.result ?? '*' }}</span>
          <span
            v-if="g.date"
            class="text-muted"
          > · {{ g.date }}</span>
        </li>
      </ul>
    </section>
  </div>
</template>
