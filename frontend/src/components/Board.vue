<script setup lang="ts">
import { ref, onMounted, watch } from 'vue'
import { Chessground } from 'chessground'
import type { Config } from 'chessground/config'
import type { Api } from 'chessground/api'
import type { DrawShape } from 'chessground/draw'
import { STARTPOS_FEN, sideToMove } from '../lib/fen'
import { planBrushes } from '../lib/plansToShapes'
import { overlayBrushes } from '../lib/boardShapes'
import type { Color, Square, Dests, BoardMove, Shape } from '../types'

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
  // Keep `shapes` across position changes (pinned study plans, #61); the live
  // engine overlay leaves this false so stale arrows clear on every new move.
  persistShapes?: boolean
  // Study editor: render `shapes` as user-editable drawings (right-click drag)
  // and emit `drawn` whenever they change, so the parent can persist them.
  // Off for the read-only engine overlay (auto-shapes that swallow no input).
  editableShapes?: boolean
  // Extra read-only auto-shapes drawn alongside editable `shapes` (study mode):
  // the live engine PV overlay, which must not clobber the pinned-plan drawings.
  overlayShapes?: DrawShape[]
}
const props = withDefaults(defineProps<Props>(), {
  fen: STARTPOS_FEN,
  orientation: 'white',
  dests: null,
  movable: false,
  lastMove: null,
  boardTheme: 'brown',
  shapes: () => [],
  persistShapes: false,
  editableShapes: false,
  overlayShapes: () => [],
})
const emit = defineEmits<{ move: [payload: BoardMove]; drawn: [shapes: Shape[]] }>()

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
    // Register the plan brushes; right-click user drawings stay enabled. In the
    // study editor those drawings are the pinned plan, so capture every change.
    drawable: {
      brushes: { ...planBrushes(), ...overlayBrushes() } as unknown as import('chessground/draw').DrawBrushes,
      onChange: props.editableShapes
        ? (shapes: DrawShape[]) => emit('drawn', shapes as unknown as Shape[])
        : undefined,
    },
  }
}

// Render `props.shapes` either as editable user drawings (study editor) or as a
// read-only auto-shape overlay (engine plans). `setShapes` is programmatic and
// never re-fires `onChange`, so this can't loop back into a `drawn` emit.
function renderShapes() {
  if (!cg) return
  if (props.editableShapes) {
    cg.setShapes(props.shapes ?? [])
    cg.setAutoShapes(props.overlayShapes ?? [])
  } else {
    cg.setAutoShapes(props.shapes ?? [])
  }
}

onMounted(() => {
  if (el.value) {
    cg = Chessground(el.value, config())
    renderShapes()
  }
})

watch(
  () => [props.fen, props.orientation, props.dests, props.movable, props.lastMove],
  () => cg && cg.set(config()),
  { deep: true },
)

// Redraw pinned/overlay shapes when either layer changes.
watch(() => props.shapes, renderShapes, { deep: true })
watch(() => props.overlayShapes, renderShapes, { deep: true })

// Clear the overlay immediately on a new position so stale plans never linger —
// unless shapes are pinned (study mode), where each node's plan is authoritative.
watch(
  () => props.fen,
  () => {
    if (props.editableShapes || props.persistShapes) renderShapes()
    else if (cg) cg.setAutoShapes([])
  },
)

// Clear the user's right-click-drawn arrows (issue #123). Leaves the computed
// auto-shape layers (plans / threats / master) intact — those are toggled off
// via their layer switches, not this control.
function clearUserShapes() {
  cg?.setShapes([])
}

defineExpose({ clearUserShapes })
</script>

<template>
  <div
    ref="el"
    class="aspect-square w-[480px] max-w-full"
    :class="`board-${boardTheme}`"
  />
</template>
