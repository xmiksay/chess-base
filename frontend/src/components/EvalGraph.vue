<script setup lang="ts">
// Compact eval sparkline for a reviewed game (issue #119, Mode A). Plots each
// reviewed ply's White-perspective eval (centipawns, mate saturating) as an SVG
// point, with a zero baseline and the current ply marked. Clicking a point jumps
// the board: emits `select(ply)`.
import { computed } from 'vue'
import type { MoveReview } from '../types'

interface Props {
  moves: MoveReview[]
  currentPly?: number
}
const props = withDefaults(defineProps<Props>(), { currentPly: 0 })
const emit = defineEmits<{ select: [ply: number] }>()

const WIDTH = 320
const HEIGHT = 80
const CLAMP = 1000 // ±10.00 pawns; mate saturates here too

interface Point {
  ply: number
  x: number
  y: number
  cp: number
}

const points = computed<Point[]>(() => {
  const n = props.moves.length
  if (n === 0) return []
  return props.moves.map((m, i) => {
    const cp = Math.max(-CLAMP, Math.min(CLAMP, m.eval_cp))
    const x = n === 1 ? WIDTH / 2 : (i / (n - 1)) * WIDTH
    // White advantage (positive cp) sits above the midline.
    const y = HEIGHT / 2 - (cp / CLAMP) * (HEIGHT / 2)
    return { ply: m.ply, x, y, cp }
  })
})

/** Polyline path of the eval curve. */
const path = computed(() => points.value.map((p) => `${p.x},${p.y}`).join(' '))

const zeroY = HEIGHT / 2

const currentPoint = computed(
  () => points.value.find((p) => p.ply === props.currentPly) ?? null,
)
</script>

<template>
  <svg
    data-test="eval-graph"
    :viewBox="`0 0 ${WIDTH} ${HEIGHT}`"
    class="h-20 w-full rounded bg-neutral-800"
    preserveAspectRatio="none"
    role="img"
    aria-label="Evaluation graph"
  >
    <!-- Zero baseline. -->
    <line
      :x1="0"
      :y1="zeroY"
      :x2="WIDTH"
      :y2="zeroY"
      stroke="rgb(115 115 115)"
      stroke-width="1"
    />
    <polyline
      v-if="points.length > 1"
      :points="path"
      fill="none"
      stroke="rgb(244 244 245)"
      stroke-width="1.5"
    />
    <!-- Marker for the current ply. -->
    <line
      v-if="currentPoint"
      :x1="currentPoint.x"
      :y1="0"
      :x2="currentPoint.x"
      :y2="HEIGHT"
      stroke="rgb(250 204 21)"
      stroke-width="1"
    />
    <circle
      v-for="p in points"
      :key="p.ply"
      data-test="eval-point"
      :cx="p.x"
      :cy="p.y"
      r="3"
      :fill="p.ply === currentPly ? 'rgb(250 204 21)' : 'rgb(163 163 163)'"
      class="cursor-pointer"
      @click="emit('select', p.ply)"
    />
  </svg>
</template>
