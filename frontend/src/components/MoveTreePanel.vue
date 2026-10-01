<script setup lang="ts">
// The notation panel: a bounded, scrolling list of depth-indented move rows,
// with node actions on a context menu.
//
// Replaces MoveTree + MoveTreeLine. What changed and why:
//   * bounded + scrolling, always — an uncapped list pushed the board and the
//     engine panel off-screen on any long game
//   * the current move is scrolled into view — nothing in the app did this
//   * variation depth is legible past level 1 (see MoveTreeRow)
//   * comments read inline instead of hiding behind a dot
//   * promote/demote/delete are a menu, never inserted into the move text
import { computed, ref } from 'vue'
import MoveTreeRow from './MoveTreeRow.vue'
import NodeContextMenu from './NodeContextMenu.vue'
import { treeRows } from '../lib/moveTreeRows'
import { useScrollIntoView } from '../lib/useScrollIntoView'
import { getNode, siblingIndex } from '../lib/moveTree'
import type { MoveTree } from '../types'

interface Props {
  tree?: MoveTree | null
  currentId?: number | null
  /** Study mode: the menu offers promote / demote / delete / comment. */
  editable?: boolean
  inlineComments?: boolean
  /** Offer Copy FEN / Copy line as PGN — needs the host to handle both. */
  copyActions?: boolean
}
const props = withDefaults(defineProps<Props>(), {
  tree: null,
  currentId: null,
  editable: false,
  inlineComments: true,
  copyActions: false,
})

// Call-signature form so `act` can emit a union event name — the object form
// makes each event its own overload, which a union argument can't satisfy.
const emit = defineEmits<{
  (
    e: 'select' | 'promote' | 'demote' | 'remove' | 'comment' | 'copy-fen' | 'copy-pgn',
    nodeId: number,
  ): void
}>()

const rows = computed(() => treeRows(props.tree))

const scroller = ref<HTMLElement | null>(null)
// Set for exactly one update when the selection came from a click in here, so
// the panel doesn't scroll away from what the user just pointed at.
const clickedHere = ref(false)
useScrollIntoView({
  container: scroller,
  currentId: computed(() => props.currentId),
  suppress: () => {
    const was = clickedHere.value
    clickedHere.value = false
    return was
  },
})

function onSelect(id: number): void {
  clickedHere.value = true
  emit('select', id)
}

const menu = ref<{ id: number; x: number; y: number } | null>(null)

const menuSan = computed(() => {
  if (!menu.value || !props.tree) return ''
  return getNode(props.tree, menu.value.id)?.san ?? ''
})

/** A node already first among its siblings has nowhere to be promoted to. */
const menuCanPromote = computed(() => {
  if (!menu.value || !props.tree) return false
  return siblingIndex(props.tree, menu.value.id) > 0
})

function onContext(payload: { id: number; x: number; y: number }): void {
  // Select what was right-clicked, so the menu and the board agree.
  clickedHere.value = true
  emit('select', payload.id)
  menu.value = payload
}

function act(event: 'promote' | 'demote' | 'remove' | 'comment' | 'copy-fen' | 'copy-pgn'): void {
  const id = menu.value?.id
  menu.value = null
  if (id != null) emit(event, id)
}
</script>

<template>
  <div
    ref="scroller"
    class="overflow-y-auto text-sm leading-relaxed"
    data-test="move-tree"
  >
    <p
      v-if="!rows.length"
      class="text-muted"
    >
      No moves yet — play a move on the board to start the line.
    </p>

    <MoveTreeRow
      v-for="row in rows"
      :key="row.key"
      v-memo="[row, row.cells.some((c) => c.id === currentId) ? currentId : null]"
      :row="row"
      :current-id="currentId"
      :inline-comments="inlineComments"
      @select="onSelect"
      @context="onContext"
    />

    <NodeContextMenu
      v-if="menu"
      :x="menu.x"
      :y="menu.y"
      :san="menuSan"
      :editable="editable"
      :can-promote="menuCanPromote"
      :copy-actions="copyActions"
      @promote="act('promote')"
      @demote="act('demote')"
      @remove="act('remove')"
      @comment="act('comment')"
      @copy-fen="act('copy-fen')"
      @copy-pgn="act('copy-pgn')"
      @close="menu = null"
    />
  </div>
</template>
