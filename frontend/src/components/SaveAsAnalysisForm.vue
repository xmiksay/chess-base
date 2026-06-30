<script setup lang="ts">
// Save-as-analysis flow (issue #164): persist the open game as a study, optionally
// filed under a folder and engine-analysed. A small inline dialog with a name, a
// folder select and an "Run engine analysis" checkbox (only when the engine is
// available). Drives the games store (which owns the open game + linked list) and
// the folders store for the select. Kept separate so GameReviewPanel stays lean.
import { onMounted, ref } from 'vue'
import { useGamesStore } from '../stores/games'
import { useFoldersStore } from '../stores/folders'

const props = defineProps<{ engineEnabled: boolean | null }>()

const games = useGamesStore()
const folders = useFoldersStore()

const open = ref(false)
const name = ref('')
const folderId = ref<number | null>(null)
const analyse = ref(false)
const depth = ref<number | null>(null)
const saving = ref(false)
const error = ref<string | null>(null)
const savedName = ref<string | null>(null)

onMounted(() => {
  if (!folders.list.length) folders.refresh().catch(() => {})
})

/** Default the name from the players (falling back to the game id). */
function defaultName(): string {
  const g = games.openGame
  if (!g) return 'Analysis'
  const white = g.white ?? '?'
  const black = g.black ?? '?'
  return `${white} – ${black}`
}

function show() {
  name.value = defaultName()
  folderId.value = null
  analyse.value = false
  error.value = null
  savedName.value = null
  open.value = true
}

async function submit() {
  if (!name.value.trim()) return
  saving.value = true
  error.value = null
  try {
    const study = await games.saveAsStudy({
      name: name.value.trim(),
      folder_id: folderId.value,
      analyse: analyse.value,
      ...(analyse.value && depth.value != null ? { depth: depth.value } : {}),
    })
    savedName.value = study?.name ?? name.value.trim()
    open.value = false
  } catch (e) {
    error.value = String((e as Error)?.message ?? e)
  } finally {
    saving.value = false
  }
}
</script>

<template>
  <span class="inline-flex items-center gap-2">
    <button
      type="button"
      data-test="save-as-analysis"
      class="rounded border border-neutral-300 px-3 py-1 text-sm hover:bg-neutral-100"
      @click="show"
    >
      Save as analysis
    </button>
    <span
      v-if="savedName"
      class="text-xs text-green-700"
      data-test="save-as-analysis-saved"
    >
      Saved “{{ savedName }}”.
    </span>

    <div
      v-if="open"
      class="absolute z-10 mt-2 w-72 rounded border border-neutral-300 bg-white p-3 shadow-lg"
      data-test="save-as-analysis-dialog"
    >
      <label class="mb-2 block text-xs text-neutral-600">
        Name
        <input
          v-model="name"
          data-test="save-as-analysis-name"
          class="mt-1 w-full rounded border border-neutral-300 px-2 py-1 text-sm"
        >
      </label>

      <label class="mb-2 block text-xs text-neutral-600">
        Folder
        <select
          v-model="folderId"
          data-test="save-as-analysis-folder"
          class="mt-1 w-full rounded border border-neutral-300 px-2 py-1 text-sm"
        >
          <option :value="null">
            Unfiled
          </option>
          <option
            v-for="f in folders.list"
            :key="f.id"
            :value="f.id"
          >
            {{ f.name }}
          </option>
        </select>
      </label>

      <label class="mb-2 flex items-center gap-2 text-xs text-neutral-600">
        <input
          v-model="analyse"
          type="checkbox"
          data-test="save-as-analysis-analyse"
          :disabled="props.engineEnabled !== true"
        >
        Run engine analysis
        <span
          v-if="props.engineEnabled !== true"
          class="text-neutral-400"
        >(no engine)</span>
      </label>

      <p
        v-if="error"
        class="mb-2 text-xs text-red-600"
        data-test="save-as-analysis-error"
      >
        {{ error }}
      </p>

      <div class="flex justify-end gap-2">
        <button
          type="button"
          class="rounded px-2 py-1 text-xs text-neutral-500 hover:text-neutral-800"
          @click="open = false"
        >
          Cancel
        </button>
        <button
          type="button"
          data-test="save-as-analysis-submit"
          class="rounded bg-neutral-800 px-3 py-1 text-xs text-white hover:bg-neutral-700 disabled:opacity-50"
          :disabled="saving || !name.trim()"
          @click="submit"
        >
          {{ saving ? 'Saving…' : 'Save' }}
        </button>
      </div>
    </div>
  </span>
</template>
