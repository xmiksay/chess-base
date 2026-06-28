<script setup>
import { ref, onMounted, watch } from 'vue'
import { Chessground } from 'chessground'
import { STARTPOS_FEN, sideToMove } from '../lib/fen.js'

// Presentational board: the parent owns position/legality and supplies the
// legal-move `dests`; the board emits the user's drag as a `move` event.
const props = defineProps({
  fen: { type: String, default: STARTPOS_FEN },
  orientation: { type: String, default: 'white' },
  dests: { type: Object, default: null }, // Map: from-square → [to-squares]
  movable: { type: Boolean, default: false },
  lastMove: { type: Array, default: null }, // [from, to]
  boardTheme: { type: String, default: 'brown' }, // see style.css `.board-*`
})
const emit = defineEmits(['move'])

const el = ref(null)
let cg = null

function placementFen(fen) {
  // chessground wants only the piece-placement field.
  return fen.split(/\s+/)[0]
}

function config() {
  return {
    fen: placementFen(props.fen),
    turnColor: sideToMove(props.fen),
    orientation: props.orientation,
    lastMove: props.lastMove || undefined,
    coordinates: true,
    movable: {
      free: false,
      color: props.movable ? sideToMove(props.fen) : undefined,
      dests: props.dests || new Map(),
      events: { after: (from, to) => emit('move', { from, to }) },
    },
  }
}

onMounted(() => {
  cg = Chessground(el.value, config())
})

watch(
  () => [props.fen, props.orientation, props.dests, props.movable, props.lastMove],
  () => cg && cg.set(config()),
  { deep: true },
)
</script>

<template>
  <div
    ref="el"
    class="aspect-square w-full max-w-[480px]"
    :class="`board-${boardTheme}`"
  />
</template>
