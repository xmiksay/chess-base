<script setup lang="ts">
// Variation tree panel: flattens the MoveTree into tokens, folds them into nested
// variation blocks, and renders them with each variation indented one level (via
// the recursive MoveTreeLine). The parent owns selection and the store edits;
// clicking a move emits `select`, and the per-node toolbar emits promote / demote
// / remove (only shown when `editable`).
import { computed } from 'vue'
import { treeTokens, tokenBlocks } from '../lib/moveTree'
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
</script>

<template>
  <div
    class="flex flex-wrap items-baseline gap-x-0.5 gap-y-1 text-sm leading-relaxed"
    data-test="move-tree"
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
      :editable="editable"
      @select="$emit('select', $event)"
      @promote="$emit('promote', $event)"
      @demote="$emit('demote', $event)"
      @remove="$emit('remove', $event)"
    />
  </div>
</template>
