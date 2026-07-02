<script setup lang="ts">
// The game "thermometer" that hugs the left edge of the board: a full-height
// EvalBar driven by the singleton engine store's top line. Like EnginePanel, it
// formats against the position the engine actually searched (`analysedFen`), not
// the live board, so the fill never inverts after a play-mode reply. `fen` is the
// board's current position, used only as the fallback before the first search.
import { computed } from 'vue'
import { useEngineStore } from '../stores/engine'
import { sideToMove } from '../lib/fen'
import EvalBar from './EvalBar.vue'

const props = defineProps<{ fen: string }>()
const engine = useEngineStore()

const evalFen = computed(() => engine.analysedFen ?? props.fen)
const stm = computed(() => sideToMove(evalFen.value))
const score = computed(() => engine.lines[0]?.score ?? null)
</script>

<template>
  <EvalBar
    :score="score"
    :side-to-move="stm"
  />
</template>
