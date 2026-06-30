<script setup lang="ts">
// Studies sidebar (issue #164): the folder tree (root "Unfiled" bucket + the
// recursive FolderTree), the studies filed under the selected folder, a per-study
// "Move to…" control, and the create-study form (which files the new study into
// the selected folder). Split out of StudyView to keep that view under the file
// cap. Drives the studies / folders stores directly; opening a study is delegated
// to the study-editor store via the `open` event the parent already wires up.
import { computed, ref, watch } from 'vue'
import FolderTree from './FolderTree.vue'
import { useStudiesStore } from '../stores/studies'
import { useFoldersStore } from '../stores/folders'
import type { Database } from '../types'

const props = defineProps<{
  databases: Database[]
  currentId: number | null
  defaultDbId: number | null
}>()

const emit = defineEmits<{
  (e: 'open', id: number): void
  (e: 'error', message: string): void
}>()

const studies = useStudiesStore()
const folders = useFoldersStore()

// null ⇒ root / "Unfiled".
const selectedFolderId = ref<number | null>(null)
const newName = ref('')
const newDb = ref<number | null>(null)
const newRootFolder = ref('')
const showNewRootFolder = ref(false)

// Studies filed under the selected folder (null ⇒ the unfiled ones).
const visibleStudies = computed(() =>
  studies.list.filter((s) => (s.folder_id ?? null) === selectedFolderId.value),
)

defineExpose({ selectedFolderId })

function fail(e: unknown) {
  emit('error', String((e as Error)?.message ?? e))
}

/** Reset to root and surface the unfiled studies after a delete. */
async function onFolderDeleted(id: number) {
  if (selectedFolderId.value === id) selectedFolderId.value = null
  await studies.refresh()
}

async function onMoveStudy(id: number, folderId: number | null) {
  try {
    await studies.setFolder(id, folderId)
  } catch (e) {
    fail(e)
  }
}

async function onCreateRootFolder() {
  const name = newRootFolder.value.trim()
  if (!name) return
  try {
    await folders.create(name, null)
  } catch (e) {
    fail(e)
  }
  newRootFolder.value = ''
  showNewRootFolder.value = false
}

async function onCreate() {
  if (!newName.value.trim() || newDb.value == null) return
  try {
    const study = await studies.create(newDb.value, newName.value.trim())
    emit('open', study.id)
    if (selectedFolderId.value != null) await studies.setFolder(study.id, selectedFolderId.value)
    newName.value = ''
    await studies.refresh()
  } catch (e) {
    fail(e)
  }
}

// Preselect the parent-provided default database once it (or the list) arrives.
watch(
  () => [props.defaultDbId, props.databases] as const,
  () => {
    if (newDb.value == null) newDb.value = props.defaultDbId ?? props.databases[0]?.id ?? null
  },
  { immediate: true },
)
</script>

<template>
  <section class="lg:w-1/4">
    <ul class="mb-2 flex flex-col gap-0.5">
      <li>
        <button
          type="button"
          data-test="folder-root"
          class="w-full rounded px-2 py-0.5 text-left text-sm hover:bg-neutral-100"
          :class="{ 'bg-neutral-100 font-medium': selectedFolderId === null }"
          @click="selectedFolderId = null"
        >
          Unfiled
        </button>
      </li>
      <FolderTree
        v-for="root in folders.childrenOf(null)"
        :key="root.id"
        :folder="root"
        :selected-id="selectedFolderId"
        @select="selectedFolderId = $event"
        @deleted="onFolderDeleted"
      />
    </ul>

    <div class="mb-4">
      <button
        v-if="!showNewRootFolder"
        type="button"
        data-test="new-root-folder"
        class="text-xs text-neutral-500 hover:text-neutral-800"
        @click="showNewRootFolder = true"
      >
        + New folder
      </button>
      <form
        v-else
        class="flex items-center gap-1"
        @submit.prevent="onCreateRootFolder"
      >
        <input
          v-model="newRootFolder"
          placeholder="Folder name"
          data-test="new-root-folder-input"
          class="flex-1 rounded border border-neutral-300 px-1 py-0.5 text-sm"
          @keyup.esc="showNewRootFolder = false"
        >
        <button
          type="submit"
          data-test="new-root-folder-submit"
          class="text-xs text-neutral-500 hover:text-neutral-800"
        >
          Add
        </button>
      </form>
    </div>

    <ul class="mb-4 flex flex-col gap-1">
      <li
        v-for="s in visibleStudies"
        :key="s.id"
      >
        <div class="flex items-center gap-1">
          <button
            type="button"
            data-test="study-row"
            class="flex-1 truncate rounded px-2 py-1 text-left text-sm hover:bg-neutral-100"
            :class="{ 'bg-neutral-100 font-medium': currentId === s.id }"
            @click="emit('open', s.id)"
          >
            {{ s.name }}{{ s.global ? ' (global)' : '' }}
          </button>
          <select
            :value="s.folder_id ?? ''"
            data-test="move-study"
            aria-label="Move to folder"
            class="rounded border border-neutral-300 px-1 py-0.5 text-xs"
            @change="onMoveStudy(s.id, ($event.target as HTMLSelectElement).value === '' ? null : Number(($event.target as HTMLSelectElement).value))"
          >
            <option value="">
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
        </div>
      </li>
    </ul>

    <form
      data-test="create-form"
      class="flex flex-col gap-2"
      @submit.prevent="onCreate"
    >
      <input
        v-model="newName"
        placeholder="New study name"
        class="rounded border border-neutral-300 px-2 py-1 text-sm"
      >
      <select
        v-model="newDb"
        aria-label="Database"
        class="rounded border border-neutral-300 px-2 py-1 text-sm"
      >
        <option
          v-for="d in databases"
          :key="d.id"
          :value="d.id"
        >
          {{ d.name }}{{ d.global ? ' (global)' : '' }}
        </option>
      </select>
      <button
        type="submit"
        class="rounded bg-neutral-800 px-3 py-1 text-sm text-white hover:bg-neutral-700 disabled:opacity-50"
        :disabled="!newName.trim() || newDb == null"
      >
        Create study
      </button>
    </form>
  </section>
</template>
