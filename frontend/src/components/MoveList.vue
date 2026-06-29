<script setup lang="ts">
// Flat game notation for the analysis board: renders a SAN history as numbered
// move pairs (1. e4 e5 2. Nf3 …). Clicking a move emits `select(ply)` where
// `ply` is the position after that move (1-based); the current ply is highlighted.
// Unlike MoveTree this has no variations — it mirrors the game store's flat line.
import { computed } from 'vue'

interface Props {
  history: string[]
  currentPly?: number
}
const props = withDefaults(defineProps<Props>(), { currentPly: 0 })
const emit = defineEmits<{ select: [ply: number] }>()

interface Move {
  san: string
  ply: number
}
interface Row {
  number: number
  white: Move | null
  black: Move | null
}

const rows = computed<Row[]>(() => {
  const out: Row[] = []
  props.history.forEach((san, i) => {
    const move: Move = { san, ply: i + 1 }
    if (i % 2 === 0) out.push({ number: i / 2 + 1, white: move, black: null })
    else out[out.length - 1].black = move
  })
  return out
})
</script>

<template>
  <div
    class="text-sm"
    data-test="move-list"
  >
    <p
      v-if="!history.length"
      class="text-neutral-500"
    >
      No moves yet — play a move on the board to start the game.
    </p>

    <ol
      v-else
      class="flex flex-wrap items-baseline gap-x-1 gap-y-0.5 leading-7"
    >
      <li
        v-for="row in rows"
        :key="row.number"
        class="flex items-baseline gap-1"
      >
        <span class="tabular-nums text-neutral-400">{{ row.number }}.</span>
        <button
          type="button"
          data-test="move"
          class="rounded px-1 hover:bg-neutral-200"
          :class="row.white && row.white.ply === currentPly ? 'bg-yellow-200 font-medium hover:bg-yellow-200' : ''"
          @click="emit('select', row.white!.ply)"
        >
          {{ row.white!.san }}
        </button>
        <button
          v-if="row.black"
          type="button"
          data-test="move"
          class="rounded px-1 hover:bg-neutral-200"
          :class="row.black.ply === currentPly ? 'bg-yellow-200 font-medium hover:bg-yellow-200' : ''"
          @click="emit('select', row.black.ply)"
        >
          {{ row.black.san }}
        </button>
      </li>
    </ol>
  </div>
</template>
