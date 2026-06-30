<script setup lang="ts">
// Shared board toolbar (issue #134): move-list navigation (⏮◀▶⏭) plus the
// board-overlay toggle row (#123) and a clear-arrows control. Navigation is
// emitted to the parent (each board page owns its own cursor); the overlay
// toggles read/write `stores/settings` directly since they are global.
import { useSettingsStore } from '../stores/settings'

defineProps<{ atStart: boolean; atEnd: boolean }>()
defineEmits<{
  first: []
  prev: []
  next: []
  last: []
  'clear-arrows': []
}>()

const settings = useSettingsStore()

/** Toggle one overlay layer and persist the choice to user settings. */
function toggleLayer(key: 'showPlans' | 'showThreats' | 'showMasterMoves', value: boolean) {
  settings.update({ [key]: value })
}
</script>

<template>
  <div>
    <!-- Move-list navigation -->
    <div class="flex items-center gap-2">
      <button
        class="rounded border border-neutral-300 px-2 py-1 text-sm disabled:opacity-50"
        :disabled="atStart"
        aria-label="Start"
        @click="$emit('first')"
      >
        ⏮
      </button>
      <button
        class="rounded border border-neutral-300 px-2 py-1 text-sm disabled:opacity-50"
        :disabled="atStart"
        aria-label="Back"
        @click="$emit('prev')"
      >
        ◀
      </button>
      <button
        class="rounded border border-neutral-300 px-2 py-1 text-sm disabled:opacity-50"
        :disabled="atEnd"
        aria-label="Forward"
        @click="$emit('next')"
      >
        ▶
      </button>
      <button
        class="rounded border border-neutral-300 px-2 py-1 text-sm disabled:opacity-50"
        :disabled="atEnd"
        aria-label="End"
        @click="$emit('last')"
      >
        ⏭
      </button>
    </div>

    <!-- Board-overlay layers (issue #123): independent, persisted toggles plus
         a control to clear hand-drawn arrows. -->
    <div class="mt-3 flex flex-wrap items-center gap-x-4 gap-y-2 text-sm">
      <label class="flex items-center gap-1.5">
        <input
          type="checkbox"
          :checked="settings.showPlans"
          data-test="toggle-plans"
          @change="toggleLayer('showPlans', ($event.target as HTMLInputElement).checked)"
        >
        <span class="text-green-700">Plans</span>
      </label>
      <label class="flex items-center gap-1.5">
        <input
          type="checkbox"
          :checked="settings.showThreats"
          data-test="toggle-threats"
          @change="toggleLayer('showThreats', ($event.target as HTMLInputElement).checked)"
        >
        <span class="text-red-600">Threats</span>
      </label>
      <label class="flex items-center gap-1.5">
        <input
          type="checkbox"
          :checked="settings.showMasterMoves"
          data-test="toggle-master"
          @change="toggleLayer('showMasterMoves', ($event.target as HTMLInputElement).checked)"
        >
        <span class="text-violet-600">Master moves</span>
      </label>
      <button
        class="ml-auto rounded border border-neutral-300 px-2 py-1 text-xs"
        data-test="clear-arrows"
        @click="$emit('clear-arrows')"
      >
        Clear arrows
      </button>
    </div>
  </div>
</template>
