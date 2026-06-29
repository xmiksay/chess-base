<script setup lang="ts">
import { computed } from 'vue'
import { evalBarPercent } from '../lib/engineStream'
import type { Color, Score } from '../types'

// Vertical eval bar: White's share fills from the bottom.
interface Props {
  score?: Score | null
  sideToMove?: Color
}
const props = withDefaults(defineProps<Props>(), {
  score: null,
  sideToMove: 'white',
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
