<script setup lang="ts">
// Recursive renderer for one level of the move tree (issue: tab-shifted PGN).
// Move items flow inline; a variation `block` item breaks to its own indented
// row (a left border + padding) and recurses, so nesting depth = visual indent.
// Presentational: the parent owns selection and the store edits.
//
// Deliberately action-free. Node actions live in MoveTree's sticky bar — an
// inline toolbar after the selected move reflows the rest of the line under the
// cursor, which turns the next click into an accidental delete.
import { nagGlyph, nagClass } from '../lib/moveTree'
import { formatEval } from '../lib/dangerShapes'
import type { MoveTreeItem } from '../lib/moveTree'

interface Props {
  items: MoveTreeItem[]
  currentId?: number | null
}
withDefaults(defineProps<Props>(), { currentId: null })

const emit = defineEmits<{
  select: [id: number]
}>()
</script>

<template>
  <template
    v-for="(item, i) in items"
    :key="i"
  >
    <!-- A move: a plain inline button. Nothing is ever inserted beside it. -->
    <span
      v-if="item.kind === 'move'"
      class="inline-flex items-baseline"
    >
      <button
        type="button"
        data-test="move"
        :data-node-id="item.token.id"
        class="rounded px-0.5 hover:bg-surface-2"
        :class="[
          item.token.depth === 0 ? 'font-medium text-fg' : 'text-muted',
          item.token.id === currentId ? 'bg-accent/15 text-fg ring-1 ring-accent hover:bg-accent/15' : '',
        ]"
        @click="emit('select', item.token.id)"
        @contextmenu.prevent="emit('select', item.token.id)"
      >
        <span
          v-if="item.token.number"
          class="mr-0.5 text-muted"
        >{{ item.token.number }}</span>{{ item.token.san
        }}<span
          v-for="(n, ni) in item.token.nags"
          :key="ni"
          :class="nagClass(n)"
        >{{ nagGlyph(n) }}</span><span
          v-if="item.token.eval"
          class="ml-0.5 text-[10px] text-muted"
          data-test="move-eval"
        >{{ formatEval(item.token.eval) }}</span><span
          v-if="item.token.comment"
          class="ml-0.5 text-good"
          data-test="comment-marker"
          title="has comment"
        >•</span>
      </button>
    </span>

    <!-- A variation: indented block on its own row, recursing one level deeper. -->
    <div
      v-else
      class="flex basis-full flex-wrap items-baseline gap-x-0.5 border-l border-border pl-2"
      data-test="variation"
    >
      <MoveTreeLine
        :items="item.items"
        :current-id="currentId"
        @select="emit('select', $event)"
      />
    </div>
  </template>
</template>
