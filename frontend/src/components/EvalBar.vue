<script setup>
import { computed } from 'vue'
import { evalBarPercent } from '../lib/engineStream.js'

// Vertical eval bar: White's share fills from the bottom.
const props = defineProps({
  score: { type: Object, default: null },
  sideToMove: { type: String, default: 'white' },
})

const whitePct = computed(() => evalBarPercent(props.score, props.sideToMove))
</script>

<template>
  <div class="relative h-full min-h-[120px] w-4 overflow-hidden rounded bg-neutral-800">
    <div
      class="absolute bottom-0 left-0 w-full bg-neutral-100 transition-[height] duration-200"
      :style="{ height: whitePct + '%' }"
    />
  </div>
</template>
