<script setup>
// Presentational variation tree: flattens the MoveTree into move / paren tokens
// and renders them inline. The parent owns selection; clicking a move emits
// `select(nodeId)`. Variations render dimmer the deeper they nest.
import { computed } from 'vue'
import { treeTokens, nagGlyph } from '../lib/moveTree.js'

const props = defineProps({
  tree: { type: Object, default: null },
  currentId: { type: Number, default: null },
})
const emit = defineEmits(['select'])

const tokens = computed(() => treeTokens(props.tree))
</script>

<template>
  <div
    class="flex flex-wrap items-baseline gap-x-1 gap-y-1 text-sm leading-6"
    data-test="move-tree"
  >
    <p
      v-if="!tokens.length"
      class="text-neutral-500"
    >
      No moves yet — play a move on the board to start the line.
    </p>

    <template
      v-for="(t, i) in tokens"
      :key="i"
    >
      <span
        v-if="t.type === 'open'"
        class="text-neutral-400"
      >(</span>
      <span
        v-else-if="t.type === 'close'"
        class="text-neutral-400"
      >)</span>
      <button
        v-else
        type="button"
        data-test="move"
        class="rounded px-1 hover:bg-neutral-200"
        :class="[
          t.depth === 0 ? 'font-medium text-neutral-900' : 'text-neutral-600',
          t.id === currentId ? 'bg-yellow-200 hover:bg-yellow-200' : '',
        ]"
        @click="emit('select', t.id)"
      >
        <span
          v-if="t.number"
          class="mr-0.5 text-neutral-400"
        >{{ t.number }}</span>{{ t.san
        }}<span
          v-for="(n, ni) in t.nags"
          :key="ni"
          class="text-blue-600"
        >{{ nagGlyph(n) }}</span>
        <span
          v-if="t.comment"
          class="ml-1 italic text-green-700"
        >{{ t.comment }}</span>
      </button>
    </template>
  </div>
</template>
