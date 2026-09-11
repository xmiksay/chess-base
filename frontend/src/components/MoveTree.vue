<script setup lang="ts">
// Variation tree panel: flattens the MoveTree into tokens, folds them into nested
// variation blocks, and renders them with each variation indented one level (via
// the recursive MoveTreeLine). The parent owns selection and the store edits;
// clicking a move emits `select`.
//
// The promote / demote / remove actions live in a sticky bar at the top of the
// panel, NOT inline after the selected move: an inline toolbar reflows every
// following move to the right the instant you select one, so the next click
// lands on the previous move's ✕ instead of the move you aimed at.
import { computed } from 'vue'
import { treeTokens, tokenBlocks, getNode } from '../lib/moveTree'
import MoveTreeLine from './MoveTreeLine.vue'
import type { MoveTree } from '../types'

interface Props {
  tree?: MoveTree | null
  currentId?: number | null
  editable?: boolean
}
const props = withDefaults(defineProps<Props>(), {
  tree: null,
  currentId: null,
  editable: false,
})
defineEmits<{
  select: [nodeId: number]
  promote: [nodeId: number]
  demote: [nodeId: number]
  remove: [nodeId: number]
}>()

const items = computed(() => tokenBlocks(treeTokens(props.tree)))

/** SAN of the selected move, or null at the root (which has no actions). */
const currentSan = computed(() => {
  if (props.currentId == null || !props.tree) return null
  return getNode(props.tree, props.currentId)?.san ?? null
})

const showActions = computed(() => props.editable && currentSan.value !== null)
</script>

<template>
  <div
    class="text-sm leading-relaxed"
    data-test="move-tree"
  >
    <div
      v-if="showActions"
      class="sticky top-0 z-10 mb-1 flex items-center gap-1 border-b border-border bg-surface py-1"
      data-test="node-actions"
    >
      <span class="mr-auto truncate text-xs text-muted">
        Selected: <span class="font-medium text-fg">{{ currentSan }}</span>
      </span>
      <button
        type="button"
        data-test="node-promote"
        class="rounded px-1.5 py-0.5 text-xs text-muted hover:bg-surface-2 hover:text-good"
        title="Promote toward mainline"
        @click="$emit('promote', currentId!)"
      >
        ⤴
      </button>
      <button
        type="button"
        data-test="node-demote"
        class="rounded px-1.5 py-0.5 text-xs text-muted hover:bg-surface-2 hover:text-warn"
        title="Demote"
        @click="$emit('demote', currentId!)"
      >
        ⤵
      </button>
      <button
        type="button"
        data-test="node-delete"
        class="rounded px-1.5 py-0.5 text-xs text-muted hover:bg-surface-2 hover:text-bad"
        title="Delete move and its line"
        @click="$emit('remove', currentId!)"
      >
        ✕
      </button>
    </div>

    <div
      class="flex flex-wrap items-baseline gap-x-0.5 gap-y-1"
      data-test="move-flow"
    >
      <p
        v-if="!items.length"
        class="text-muted"
      >
        No moves yet — play a move on the board to start the line.
      </p>

      <MoveTreeLine
        :items="items"
        :current-id="currentId"
        @select="$emit('select', $event)"
      />
    </div>
  </div>
</template>
