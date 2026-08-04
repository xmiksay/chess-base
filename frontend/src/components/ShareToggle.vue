<script setup lang="ts">
// Public-sharing control for a game/study header (issue #213, ADR-0045): a
// checkbox flips the object's `public` flag (anonymous read access), and while
// on, a "Copy link" button puts the read-only deep link on the clipboard.
// Presentational — the parent owns the API call and the object's `public` field.
import { ref } from 'vue'

interface Props {
  modelValue: boolean // the object's `public` flag
  shareUrl: string // absolute URL to the object's read-only deep link
  disabled?: boolean // toggling in flight
}
const props = withDefaults(defineProps<Props>(), { disabled: false })
const emit = defineEmits<{ 'update:modelValue': [value: boolean] }>()

const copied = ref(false)

async function copyLink() {
  try {
    await navigator.clipboard.writeText(props.shareUrl)
    copied.value = true
    window.setTimeout(() => (copied.value = false), 1500)
  } catch {
    // Clipboard API unavailable (insecure context / permission denied) — the
    // link is still visible and selectable in the field for a manual copy.
  }
}
</script>

<template>
  <div
    class="flex items-center gap-2 text-sm"
    data-test="share-toggle"
  >
    <label class="flex items-center gap-1.5">
      <input
        type="checkbox"
        data-test="public-toggle"
        :checked="modelValue"
        :disabled="disabled"
        @change="emit('update:modelValue', ($event.target as HTMLInputElement).checked)"
      >
      Public
    </label>
    <template v-if="modelValue">
      <input
        readonly
        :value="shareUrl"
        data-test="share-link"
        aria-label="Share link"
        class="w-40 truncate rounded border border-border bg-surface-2 px-2 py-1 text-xs sm:w-56"
        @focus="($event.target as HTMLInputElement).select()"
      >
      <button
        type="button"
        data-test="copy-link"
        class="rounded border border-border px-2 py-1 text-xs hover:bg-surface-2"
        @click="copyLink"
      >
        {{ copied ? 'Copied!' : 'Copy link' }}
      </button>
    </template>
  </div>
</template>
