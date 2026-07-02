<script setup lang="ts">
// Read-only comment box for the selected move, sat below the board. Takes the
// same inputs as MoveTree (tree + current id) and resolves the node itself, so
// the move list stays compact (moves only) while the text lives here.
import { computed } from 'vue'
import { getNode, nagGlyph } from '../lib/moveTree'
import type { MoveTree } from '../types'

interface Props {
  tree?: MoveTree | null
  currentId?: number | null
}
const props = withDefaults(defineProps<Props>(), {
  tree: null,
  currentId: null,
})

const node = computed(() =>
  props.tree && props.currentId != null ? getNode(props.tree, props.currentId) : null,
)
const comment = computed(() => node.value?.comment ?? null)
const nags = computed(() => node.value?.nags ?? [])
const san = computed(() => node.value?.san ?? null)
</script>

<template>
  <div
    class="min-h-[3rem] rounded border border-border bg-surface-2 px-3 py-2 text-sm"
    data-test="move-comment"
  >
    <template v-if="comment">
      <span
        v-if="san"
        class="mr-1 font-medium text-fg"
      >{{ san
      }}<span
        v-for="(n, ni) in nags"
        :key="ni"
        class="text-accent"
      >{{ nagGlyph(n) }}</span></span>
      <span class="text-fg">{{ comment }}</span>
    </template>
    <span
      v-else
      class="text-muted"
    >No comment on this move.</span>
  </div>
</template>
