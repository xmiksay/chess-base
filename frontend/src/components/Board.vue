<script setup>
import { ref, onMounted, watch } from 'vue'
import { Chessground } from 'chessground'
import { STARTPOS_FEN, sideToMove } from '../lib/fen.js'

const props = defineProps({
  fen: { type: String, default: STARTPOS_FEN },
})

const el = ref(null)
let cg = null

function placementFen(fen) {
  // chessground wants only the piece-placement field.
  return fen.split(/\s+/)[0]
}

onMounted(() => {
  cg = Chessground(el.value, {
    fen: placementFen(props.fen),
    turnColor: sideToMove(props.fen),
    coordinates: true,
  })
})

watch(
  () => props.fen,
  (fen) => cg && cg.set({ fen: placementFen(fen), turnColor: sideToMove(fen) }),
)
</script>

<template>
  <div
    ref="el"
    class="aspect-square w-full max-w-[480px]"
  />
</template>
