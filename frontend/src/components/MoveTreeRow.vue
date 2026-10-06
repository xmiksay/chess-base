<script setup lang="ts">
// One nesting level of the notation, rendered as its own row.
//
// Three signals separate depths, because indent alone stopped being readable
// past about two levels and the old renderer had only "mainline or not":
//   1. indent        — a runtime paddingLeft, so Tailwind needn't safelist it
//   2. a colour rail — a literal class map, so Tailwind's scanner sees the names
//   3. type scale    — size and weight fall away as depth grows
//
// Deliberately action-free in the flow: node actions are a context menu, never
// something inserted beside the selected move (that reflow is what turned the
// next click into an accidental delete).
import { computed } from 'vue'
import { nagGlyph, nagClass } from '../lib/moveTree'
import { formatEval } from '../lib/dangerShapes'
import type { MoveRow } from '../lib/moveTreeRows'

interface Props {
  row: MoveRow
  currentId?: number | null
  /** Render each move's comment inline; false falls back to a dot marker. */
  inlineComments?: boolean
}
const props = withDefaults(defineProps<Props>(), {
  currentId: null,
  inlineComments: true,
})

const emit = defineEmits<{
  select: [id: number]
  context: [payload: { id: number; x: number; y: number }]
}>()

// Written out in full so Tailwind's scanner keeps these classes.
const DEPTH_RAIL: Record<number, string> = {
  1: 'border-depth-1',
  2: 'border-depth-2',
  3: 'border-depth-3',
  4: 'border-depth-4',
  5: 'border-depth-5',
}
const DEPTH_TEXT: Record<number, string> = {
  0: 'text-[15px] font-medium text-fg',
  1: 'text-[13.5px] text-fg/90',
  2: 'text-[13px] text-fg/75',
  3: 'text-[12.5px] text-muted',
  4: 'text-[12.5px] text-muted',
  5: 'text-[12px] italic text-muted',
}

/** Depth beyond 5 keeps the depth-5 treatment rather than fading to nothing. */
const level = computed(() => Math.min(props.row.depth, 5))
const railClass = computed(() => (level.value === 0 ? '' : `border-l-2 ${DEPTH_RAIL[level.value]}`))
const textClass = computed(() => DEPTH_TEXT[level.value])
const indent = computed(() => `${props.row.depth * 0.875}rem`)

function onContext(e: MouseEvent, id: number): void {
  emit('context', { id, x: e.clientX, y: e.clientY })
}
</script>

<template>
  <div
    class="flex flex-wrap items-baseline gap-x-0.5 py-0.5 pl-2"
    :class="[railClass, textClass]"
    :style="{ marginLeft: indent }"
    data-test="move-row"
    :data-depth="row.depth"
  >
    <template
      v-for="cell in row.cells"
      :key="cell.id"
    >
      <button
        type="button"
        data-test="move"
        :data-node-id="cell.id"
        class="rounded px-0.5 hover:bg-surface-2"
        :class="cell.id === currentId ? 'bg-accent/15 text-fg ring-1 ring-accent hover:bg-accent/15' : ''"
        @click="emit('select', cell.id)"
        @contextmenu.prevent="onContext($event, cell.id)"
      >
        <span
          v-if="cell.number"
          class="mr-0.5 text-muted"
        >{{ cell.number }}</span>{{ cell.san
        }}<span
          v-for="(n, ni) in cell.nags"
          :key="ni"
          :class="nagClass(n)"
        >{{ nagGlyph(n) }}</span><span
          v-if="cell.eval"
          class="ml-0.5 text-[10px] text-muted"
          data-test="move-eval"
        >{{ formatEval(cell.eval) }}</span>
      </button>

      <!-- A branch point, flagged so a variation is visible without clicking. -->
      <span
        v-if="cell.siblings > 1"
        class="text-[10px] text-muted"
        data-test="branch-badge"
        :title="`${cell.siblings} alternatives here`"
      >⌄{{ cell.siblings }}</span>

      <!-- Comments read inline: they used to be a dot you had to click. -->
      <span
        v-if="cell.comment && inlineComments"
        class="mx-1 whitespace-pre-line break-words text-[0.92em] italic text-muted"
        data-test="move-comment"
      >{{ cell.comment }}</span>
      <span
        v-else-if="cell.comment"
        class="ml-0.5 text-good"
        data-test="comment-marker"
        title="has comment"
      >•</span>
    </template>
  </div>
</template>
