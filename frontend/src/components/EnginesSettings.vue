<script setup>
import { onMounted, reactive, ref } from 'vue'
import { api } from '../api.js'

// Persisted engine registry (issue #53): list, add/edit, pick the default,
// remove. Writes are admin-gated server-side; errors surface inline.
const engines = ref([])
const defaultName = ref(null)
const error = ref(null)
const form = reactive({ name: '', path: '', runner: '', weights: '' })

async function refresh() {
  error.value = null
  try {
    engines.value = await api.engines.list()
    defaultName.value = (await api.engines.default()).default
  } catch (e) {
    error.value = String(e.message ?? e)
  }
}

async function run(action) {
  error.value = null
  try {
    await action()
    await refresh()
  } catch (e) {
    error.value = String(e.message ?? e)
  }
}

function add() {
  if (!form.name.trim() || !form.path.trim()) {
    error.value = 'name and path are required'
    return
  }
  const config = { name: form.name.trim(), path: form.path.trim() }
  if (form.runner.trim()) config.runner = form.runner.trim()
  if (form.weights.trim()) config.weights = form.weights.trim()
  run(async () => {
    await api.engines.upsert(config)
    Object.assign(form, { name: '', path: '', runner: '', weights: '' })
  })
}

const selectDefault = (name) => run(() => api.engines.setDefault(name))
const remove = (name) => run(() => api.engines.remove(name))

onMounted(refresh)

defineExpose({ refresh })
</script>

<template>
  <section class="rounded border border-neutral-200 p-4">
    <h2 class="mb-3 text-lg font-semibold">
      Engines
    </h2>

    <p
      v-if="error"
      class="mb-3 text-sm text-red-600"
      data-test="error"
    >
      {{ error }}
    </p>

    <ul
      v-if="engines.length"
      class="mb-4 divide-y divide-neutral-100"
    >
      <li
        v-for="e in engines"
        :key="e.name"
        class="flex items-center gap-3 py-2 text-sm"
        data-test="engine-row"
      >
        <input
          type="radio"
          :checked="e.name === defaultName"
          :aria-label="`Use ${e.name} by default`"
          @change="selectDefault(e.name)"
        >
        <span class="font-medium">{{ e.name }}</span>
        <span class="text-neutral-500">{{ e.runner ? `${e.runner} ` : '' }}{{ e.path }}</span>
        <button
          class="ml-auto text-red-600 hover:underline"
          @click="remove(e.name)"
        >
          Remove
        </button>
      </li>
    </ul>
    <p
      v-else
      class="mb-4 text-sm text-neutral-500"
    >
      No engines configured.
    </p>

    <form
      class="grid grid-cols-2 gap-2"
      @submit.prevent="add"
    >
      <input
        v-model="form.name"
        placeholder="Name (e.g. Stockfish 16)"
        class="rounded border border-neutral-300 px-2 py-1 text-sm"
      >
      <input
        v-model="form.path"
        placeholder="Binary path"
        class="rounded border border-neutral-300 px-2 py-1 text-sm"
      >
      <input
        v-model="form.runner"
        placeholder="Runner (optional, e.g. wine)"
        class="rounded border border-neutral-300 px-2 py-1 text-sm"
      >
      <input
        v-model="form.weights"
        placeholder="Weights (optional, Lc0/Maia)"
        class="rounded border border-neutral-300 px-2 py-1 text-sm"
      >
      <button
        type="submit"
        class="col-span-2 rounded bg-neutral-800 px-3 py-1 text-sm text-white hover:bg-neutral-700"
      >
        Add / update engine
      </button>
    </form>
  </section>
</template>
