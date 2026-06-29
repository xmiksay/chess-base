<script setup lang="ts">
import { ref, onMounted, watch } from 'vue'
import { Chessground } from 'chessground'
import type { Config } from 'chessground/config'
import type { Api } from 'chessground/api'
import type { DrawShape } from 'chessground/draw'
import { STARTPOS_FEN, sideToMove } from '../lib/fen'
import { planBrushes } from '../lib/plansToShapes'
import type { Color, Square, Dests, BoardMove } from '../types'

// Presentational board: the parent owns position/legality and supplies the
// legal-move `dests`; the board emits the user's drag as a `move` event.
interface Props {
  fen?: string
  orientation?: Color
  dests?: Dests | null // Map: from-square → [to-squares]
  movable?: boolean
  lastMove?: [Square, Square] | null // [from, to]
  boardTheme?: string // see style.css `.board-*`
  shapes?: DrawShape[] // plan-overlay auto-shapes (chessground brushes)
}
const props = withDefaults(defineProps<Props>(), {
  fen: STARTPOS_FEN,
  orientation: 'white',
  dests: null,
  movable: false,
  lastMove: null,
  boardTheme: 'brown',
  shapes: () => [],
})
const emit = defineEmits<{ move: [payload: BoardMove] }>()

const el = ref<HTMLElement | null>(null)
let cg: Api | null = null

function placementFen(fen: string): string {
  // chessground wants only the piece-placement field.
  return fen.split(/\s+/)[0]
}

function config(): Config {
  return {
    fen: placementFen(props.fen),
    turnColor: sideToMove(props.fen),
    orientation: props.orientation,
    lastMove: props.lastMove
      ? (props.lastMove as import('chessground/types').Key[])
      : undefined,
    coordinates: true,
    movable: {
      free: false,
      color: props.movable ? sideToMove(props.fen) : undefined,
      dests: (props.dests || new Map()) as unknown as import('chessground/types').Dests,
      events: { after: (from, to) => emit('move', { from, to }) },
    },
    // Register the plan brushes; right-click user drawings stay enabled. The
    // overlay itself is pushed via setAutoShapes so it never clobbers them.
    drawable: { brushes: planBrushes() as unknown as import('chessground/draw').DrawBrushes },
  }
}

onMounted(() => {
  if (el.value) {
    cg = Chessground(el.value, config())
    if (props.shapes.length) cg.setAutoShapes(props.shapes)
  }
})

watch(
  () => [props.fen, props.orientation, props.dests, props.movable, props.lastMove],
  () => cg && cg.set(config()),
  { deep: true },
)

// Plan overlay: redraw auto-shapes when they change.
watch(
  () => props.shapes,
  (shapes) => cg && cg.setAutoShapes(shapes ?? []),
  { deep: true },
)

// Clear the overlay immediately on a new position so stale plans never linger.
watch(
  () => props.fen,
  () => cg && cg.setAutoShapes([]),
)
</script>

<template>
  <div
    ref="el"
    class="aspect-square w-full max-w-[480px]"
    :class="`board-${boardTheme}`"
  />
</template>
