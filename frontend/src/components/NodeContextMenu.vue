<script setup lang="ts">
// Node actions, opened by right-click on a move.
//
// This is where promote/demote/delete live now. They used to render inline in
// the move text next to the selection, which reflowed the rest of the line under
// the cursor — so the click you aimed at the next move landed on the previous
// move's ✕. A menu cannot do that: it is positioned over the text, not inserted
// into it.
//
// Delete additionally arms on a short delay, so a menu that opens under an
// already-moving cursor cannot eat a stray click.
import { computed, onMounted, onUnmounted, ref } from 'vue'

interface Props {
  x: number
  y: number
  san: string
  /** Promote/demote/delete are study-only; a game view gets the copy items. */
  editable?: boolean
  /** Root or mainline-first node: there is nothing to promote. */
  canPromote?: boolean
  /** Copy FEN / PGN need the clipboard helpers; off until the host wires them. */
  copyActions?: boolean
}
const props = withDefaults(defineProps<Props>(), {
  editable: false,
  canPromote: true,
  copyActions: false,
})

const emit = defineEmits<{
  promote: []
  demote: []
  remove: []
  comment: []
  'copy-fen': []
  'copy-pgn': []
  close: []
}>()

/** Milliseconds before Delete accepts a click — guards an in-flight cursor. */
const ARM_DELAY_MS = 250
const armed = ref(false)
let armTimer: ReturnType<typeof setTimeout> | undefined

const MENU_W = 208
const MENU_H = 260 // generous: clamping a little early is harmless

/** Clamp into the viewport so a menu opened near an edge stays fully visible. */
const style = computed(() => ({
  left: `${Math.max(4, Math.min(props.x, window.innerWidth - MENU_W - 4))}px`,
  top: `${Math.max(4, Math.min(props.y, window.innerHeight - MENU_H - 4))}px`,
}))

function onKey(e: KeyboardEvent): void {
  if (e.key === 'Escape') emit('close')
}

onMounted(() => {
  armTimer = setTimeout(() => { armed.value = true }, ARM_DELAY_MS)
  window.addEventListener('keydown', onKey)
  // `true` = capture: close before the click reaches whatever is underneath.
  window.addEventListener('mousedown', onOutside, true)
  window.addEventListener('scroll', onScroll, true)
})
onUnmounted(() => {
  clearTimeout(armTimer)
  window.removeEventListener('keydown', onKey)
  window.removeEventListener('mousedown', onOutside, true)
  window.removeEventListener('scroll', onScroll, true)
})

const root = ref<HTMLElement | null>(null)
function onOutside(e: MouseEvent): void {
  if (!root.value?.contains(e.target as Node)) emit('close')
}
function onScroll(): void {
  emit('close')
}
</script>

<template>
  <div
    ref="root"
    class="fixed z-50 w-52 rounded border border-border bg-surface py-1 text-sm shadow-lg"
    :style="style"
    data-test="node-menu"
    role="menu"
  >
    <p class="truncate px-3 py-1 text-xs text-muted">
      {{ san }}
    </p>
    <div class="my-1 border-t border-border" />

    <template v-if="editable">
      <button
        type="button"
        data-test="menu-promote"
        class="block w-full px-3 py-1.5 text-left hover:bg-surface-2 disabled:opacity-40"
        :disabled="!canPromote"
        @click="emit('promote')"
      >
        Promote toward mainline
      </button>
      <button
        type="button"
        data-test="menu-demote"
        class="block w-full px-3 py-1.5 text-left hover:bg-surface-2"
        @click="emit('demote')"
      >
        Demote
      </button>
      <button
        type="button"
        data-test="menu-comment"
        class="block w-full px-3 py-1.5 text-left hover:bg-surface-2"
        @click="emit('comment')"
      >
        Comment…
      </button>
    </template>

    <template v-if="copyActions">
      <div
        v-if="editable"
        class="my-1 border-t border-border"
      />
      <button
        type="button"
        data-test="menu-copy-fen"
        class="block w-full px-3 py-1.5 text-left hover:bg-surface-2"
        @click="emit('copy-fen')"
      >
        Copy FEN
      </button>
      <button
        type="button"
        data-test="menu-copy-pgn"
        class="block w-full px-3 py-1.5 text-left hover:bg-surface-2"
        @click="emit('copy-pgn')"
      >
        Copy line as PGN
      </button>
    </template>

    <template v-if="editable">
      <div class="my-1 border-t border-border" />
      <button
        type="button"
        data-test="menu-delete"
        class="block w-full px-3 py-1.5 text-left text-bad hover:bg-surface-2 disabled:opacity-40"
        :disabled="!armed"
        @click="emit('remove')"
      >
        Delete from here
      </button>
    </template>
  </div>
</template>
