<script setup lang="ts">
// Share control (issue #213): a "Public" toggle plus a "Copy link" button for
// the object's deep link. The server enforces who may actually toggle; the
// `canToggle` prop only hides the checkbox from callers who would always 403
// (e.g. an anonymous visitor, or a non-admin on a global object) — they still
// get the read-only "Public" pill and the copy button.
import { ref } from 'vue'

const props = defineProps<{ isPublic: boolean; canToggle: boolean; url: string }>()
const emit = defineEmits<{ toggle: [] }>()

const copied = ref(false)

async function copyLink() {
  try {
    await navigator.clipboard.writeText(props.url)
    copied.value = true
    window.setTimeout(() => (copied.value = false), 2000)
  } catch {
    // Clipboard unavailable (permissions / non-secure context): fall back to a
    // prompt pre-filled with the link so the user can copy it by hand.
    window.prompt('Copy link', props.url)
  }
}
</script>

<template>
  <div class="flex items-center gap-2 text-sm">
    <label
      v-if="canToggle"
      class="flex cursor-pointer select-none items-center gap-1"
    >
      <input
        type="checkbox"
        data-test="share-toggle"
        :checked="isPublic"
        @change="emit('toggle')"
      >
      Public
    </label>
    <span
      v-else-if="isPublic"
      data-test="share-pill"
      class="rounded-full bg-surface-2 px-2 py-0.5 text-xs text-muted"
    >Public</span>
    <button
      v-if="isPublic"
      type="button"
      data-test="copy-link"
      class="rounded border border-border px-2 py-1 text-xs hover:bg-surface-2"
      @click="copyLink"
    >
      {{ copied ? 'Copied!' : 'Copy link' }}
    </button>
  </div>
</template>
