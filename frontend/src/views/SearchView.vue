<script setup lang="ts">
// Search surface (issue #69): a tab toggle between header/metadata search and the
// position / opening-tree explorer. Each tab is its own component; shared search
// state lives in the search store.
import { ref } from 'vue'
import HeaderSearch from '../components/HeaderSearch.vue'
import PositionExplorer from '../components/PositionExplorer.vue'

const tab = ref('header') // 'header' | 'position'
</script>

<template>
  <div class="mx-auto max-w-5xl p-6">
    <h2 class="text-lg font-semibold">
      Search
    </h2>

    <div class="mt-3 mb-5 flex gap-1 border-b border-border">
      <button
        type="button"
        data-test="tab-header"
        class="border-b-2 px-3 py-2 text-sm font-medium"
        :class="tab === 'header'
          ? 'border-accent text-accent'
          : 'border-transparent text-muted hover:text-fg'"
        @click="tab = 'header'"
      >
        Header search
      </button>
      <button
        type="button"
        data-test="tab-position"
        class="border-b-2 px-3 py-2 text-sm font-medium"
        :class="tab === 'position'
          ? 'border-accent text-accent'
          : 'border-transparent text-muted hover:text-fg'"
        @click="tab = 'position'"
      >
        Position explorer
      </button>
    </div>

    <HeaderSearch v-show="tab === 'header'" />
    <PositionExplorer v-if="tab === 'position'" />
  </div>
</template>
