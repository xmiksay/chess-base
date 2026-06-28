<script setup>
import { ref, onMounted } from 'vue'
import Board from './components/Board.vue'
import { STARTPOS_FEN } from './lib/fen.js'
import { api } from './api.js'

const fen = ref(STARTPOS_FEN)
const backend = ref(null)
const error = ref(null)

onMounted(async () => {
  try {
    backend.value = await api.health()
  } catch (e) {
    error.value = String(e)
  }
})
</script>

<template>
  <div class="min-h-screen bg-neutral-50 text-neutral-900">
    <header class="border-b border-neutral-200 px-6 py-4">
      <h1 class="text-xl font-semibold">
        chess-base
      </h1>
      <p class="text-sm text-neutral-500">
        Collect, store, search and study chess games.
      </p>
    </header>

    <main class="mx-auto flex max-w-5xl flex-col gap-6 p-6 md:flex-row">
      <section>
        <Board :fen="fen" />
      </section>

      <aside class="flex-1 space-y-3">
        <h2 class="font-medium">
          Status
        </h2>
        <p
          v-if="error"
          class="text-sm text-red-600"
        >
          Backend offline: {{ error }}
        </p>
        <pre
          v-else-if="backend"
          class="rounded bg-neutral-100 p-3 text-xs"
        >{{ JSON.stringify(backend, null, 2) }}</pre>
        <p
          v-else
          class="text-sm text-neutral-500"
        >
          Connecting…
        </p>
      </aside>
    </main>
  </div>
</template>
