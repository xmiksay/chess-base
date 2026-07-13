<script setup lang="ts">
// Merge-selected-games-into-a-study dialog (issue #171): the header-search
// bulk-action counterpart of GamesView's merge flow (issue #170), but with a
// real picker instead of a `window.prompt` — fold the selection into an
// existing study (a `<select>` of `useStudiesStore().list`) or create a new one
// (name + optional folder). Both paths call the same `POST /api/studies/merge-games`.
import { computed, onMounted, ref } from 'vue'
import { api } from '../api'
import { useStudiesStore } from '../stores/studies'
import { useFoldersStore } from '../stores/folders'
import type { Study } from '../types'

const props = defineProps<{ gameIds: number[] }>()
const emit = defineEmits<{ close: []; merged: [study: Study] }>()

const studies = useStudiesStore()
const folders = useFoldersStore()

// `null` ⇒ create a new study; any other value ⇒ graft into that existing one.
const studyId = ref<number | null>(null)
const name = ref('')
const folderId = ref<number | null>(null)
const merging = ref(false)
const error = ref<string | null>(null)

const canSubmit = computed(
  () =>
    !merging.value &&
    props.gameIds.length > 0 &&
    (studyId.value != null || !!name.value.trim()),
)

onMounted(() => {
  if (!studies.list.length) studies.refresh().catch(() => {})
  if (!folders.list.length) folders.refresh().catch(() => {})
})

async function submit() {
  if (!canSubmit.value) return
  merging.value = true
  error.value = null
  try {
    const study = await api.studies.mergeGames({
      game_ids: props.gameIds,
      ...(studyId.value != null
        ? { study_id: studyId.value }
        : { name: name.value.trim(), folder_id: folderId.value }),
    })
    emit('merged', study)
  } catch (e) {
    error.value = String((e as Error)?.message ?? e)
  } finally {
    merging.value = false
  }
}
</script>

<template>
  <div
    class="fixed inset-0 z-50 flex items-center justify-center bg-black/40 p-4"
    data-test="merge-games-dialog"
    @click.self="emit('close')"
  >
    <div class="w-full max-w-sm rounded bg-surface p-5 shadow-lg">
      <header class="mb-4 flex items-center justify-between">
        <h3 class="text-base font-semibold">
          Merge {{ gameIds.length }} game{{ gameIds.length === 1 ? '' : 's' }} into a study
        </h3>
        <button
          type="button"
          class="text-muted hover:text-fg"
          aria-label="Close"
          @click="emit('close')"
        >
          ✕
        </button>
      </header>

      <form
        class="flex flex-col gap-3"
        @submit.prevent="submit"
      >
        <label class="flex flex-col gap-1 text-sm">
          Target study
          <select
            v-model="studyId"
            data-test="merge-target-study"
            class="rounded border border-border px-2 py-1"
          >
            <option :value="null">
              + New study
            </option>
            <option
              v-for="s in studies.list"
              :key="s.id"
              :value="s.id"
            >
              {{ s.name }}
            </option>
          </select>
        </label>

        <label
          v-if="studyId == null"
          class="flex flex-col gap-1 text-sm"
        >
          New study name
          <input
            v-model="name"
            data-test="merge-new-name"
            placeholder="e.g. Carlsen repertoire"
            class="rounded border border-border px-2 py-1"
          >
        </label>

        <label
          v-if="studyId == null"
          class="flex flex-col gap-1 text-sm"
        >
          Folder
          <select
            v-model="folderId"
            data-test="merge-folder"
            class="rounded border border-border px-2 py-1"
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

        <p
          v-if="error"
          class="text-xs text-bad"
          data-test="merge-error"
        >
          {{ error }}
        </p>

        <div class="mt-1 flex justify-end gap-2">
          <button
            type="button"
            class="rounded border border-border px-3 py-1 text-sm hover:bg-surface-2"
            @click="emit('close')"
          >
            Cancel
          </button>
          <button
            type="submit"
            data-test="merge-submit"
            class="rounded bg-fg px-3 py-1 text-sm text-surface hover:opacity-90 disabled:opacity-50"
            :disabled="!canSubmit"
          >
            {{ merging ? 'Merging…' : 'Merge' }}
          </button>
        </div>
      </form>
    </div>
  </div>
</template>
