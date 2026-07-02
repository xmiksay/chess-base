<script setup lang="ts">
// Editor for the selected node's annotations: free-text comment, NAG glyphs, and
// the variation actions (promote to mainline / delete subtree). Presentational —
// it emits intents and lets the parent drive the store. Disabled at the root,
// which carries no move to annotate.
import { ref, watch } from 'vue'
import { nagGlyph } from '../lib/moveTree'
import type { MoveNode } from '../types'

interface Props {
  node?: MoveNode | null
}
const props = withDefaults(defineProps<Props>(), {
  node: null,
})
const emit = defineEmits<{
  comment: [comment: string]
  nag: [nag: number]
  promote: []
  demote: []
  delete: []
}>()

// The NAGs offered as quick buttons (good/mistake/brilliant/blunder/dubious).
const NAGS = [1, 2, 3, 4, 5, 6]

const comment = ref('')
// Keep the textarea in sync when the selection changes.
watch(
  () => props.node,
  (n) => {
    comment.value = n?.comment ?? ''
  },
  { immediate: true },
)

const editable = () => props.node && props.node.parent != null
</script>

<template>
  <div
    class="flex flex-col gap-2"
    data-test="annotation-editor"
  >
    <div class="flex flex-wrap gap-1">
      <button
        v-for="n in NAGS"
        :key="n"
        type="button"
        data-test="nag"
        class="rounded border border-border px-2 py-0.5 text-sm hover:bg-surface-2 disabled:opacity-50"
        :class="{ 'border-accent bg-accent/10': node?.nags?.includes(n) }"
        :disabled="!editable()"
        @click="emit('nag', n)"
      >
        {{ nagGlyph(n) }}
      </button>
    </div>

    <textarea
      v-model="comment"
      data-test="comment"
      rows="2"
      placeholder="Comment on this move…"
      aria-label="Comment"
      class="w-full rounded border border-border bg-surface px-2 py-1 text-sm disabled:opacity-50"
      :disabled="!editable()"
      @change="emit('comment', comment)"
    />

    <div class="flex gap-2">
      <button
        type="button"
        data-test="promote"
        class="rounded border border-border px-2 py-1 text-sm hover:bg-surface-2 disabled:opacity-50"
        :disabled="!editable()"
        @click="emit('promote')"
      >
        Promote
      </button>
      <button
        type="button"
        data-test="demote"
        class="rounded border border-border px-2 py-1 text-sm hover:bg-surface-2 disabled:opacity-50"
        :disabled="!editable()"
        @click="emit('demote')"
      >
        Demote
      </button>
      <button
        type="button"
        data-test="delete"
        class="rounded border border-bad/50 px-2 py-1 text-sm text-bad hover:bg-bad/10 disabled:opacity-50"
        :disabled="!editable()"
        @click="emit('delete')"
      >
        Delete
      </button>
    </div>
  </div>
</template>
