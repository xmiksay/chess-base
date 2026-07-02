<script setup lang="ts">
// Recursive renderer for one level of the move tree (issue: tab-shifted PGN).
// Move items flow inline; a variation `block` item breaks to its own indented
// row (a left border + padding) and recurses, so nesting depth = visual indent.
// Presentational: the parent owns selection and the store edits.
import { nagGlyph, nagClass } from '../lib/moveTree'
import type { MoveTreeItem } from '../lib/moveTree'

interface Props {
  items: MoveTreeItem[]
  currentId?: number | null
  editable?: boolean
}
withDefaults(defineProps<Props>(), { currentId: null, editable: false })

const emit = defineEmits<{
  select: [id: number]
  promote: [id: number]
  demote: [id: number]
  remove: [id: number]
}>()
</script>

<template>
  <template
    v-for="(item, i) in items"
    :key="i"
  >
    <!-- A move: inline button (+ an action toolbar when it is the selection). -->
    <span
      v-if="item.kind === 'move'"
      class="inline-flex items-baseline"
    >
      <button
        type="button"
        data-test="move"
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
          v-if="item.token.comment"
          class="ml-0.5 text-good"
          data-test="comment-marker"
          title="has comment"
        >•</span>
      </button>

      <span
        v-if="editable && item.token.id === currentId"
        class="ml-1 inline-flex items-center gap-0.5"
        data-test="node-actions"
      >
        <button
          type="button"
          data-test="node-promote"
          class="rounded px-1 text-xs text-muted hover:bg-surface-2 hover:text-good"
          title="Promote toward mainline"
          @click="emit('promote', item.token.id)"
        >⤴</button>
        <button
          type="button"
          data-test="node-demote"
          class="rounded px-1 text-xs text-muted hover:bg-surface-2 hover:text-warn"
          title="Demote"
          @click="emit('demote', item.token.id)"
        >⤵</button>
        <button
          type="button"
          data-test="node-delete"
          class="rounded px-1 text-xs text-muted hover:bg-surface-2 hover:text-bad"
          title="Delete move and its line"
          @click="emit('remove', item.token.id)"
        >✕</button>
      </span>
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
        :editable="editable"
        @select="emit('select', $event)"
        @promote="emit('promote', $event)"
        @demote="emit('demote', $event)"
        @remove="emit('remove', $event)"
      />
    </div>
  </template>
</template>
