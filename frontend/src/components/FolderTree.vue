<script setup lang="ts">
// Recursive study-folder tree row (issue #164): renders one folder and its
// children, with expand/collapse, selection, and inline actions (new subfolder,
// rename, delete). The folder data lives in the folders store; this component is
// purely presentational + emits a `select` up to the view that tracks the
// selected folder and filters the studies under it.
import { ref } from 'vue'
import { useFoldersStore } from '../stores/folders'
import type { FolderSummary } from '../types'

const props = defineProps<{
  folder: FolderSummary
  selectedId: number | null
}>()

const emit = defineEmits<{
  (e: 'select', id: number): void
  (e: 'deleted', id: number): void
}>()

const folders = useFoldersStore()

const expanded = ref(true)
const renaming = ref(false)
const renameValue = ref('')
const addingChild = ref(false)
const childName = ref('')

function startRename() {
  renameValue.value = props.folder.name
  renaming.value = true
}

async function submitRename() {
  const name = renameValue.value.trim()
  if (name) await folders.rename(props.folder.id, name)
  renaming.value = false
}

async function submitChild() {
  const name = childName.value.trim()
  if (name) {
    await folders.create(name, props.folder.id)
    expanded.value = true
  }
  childName.value = ''
  addingChild.value = false
}

async function onDelete() {
  await folders.remove(props.folder.id)
  emit('deleted', props.folder.id)
}
</script>

<template>
  <li data-test="folder-node">
    <div
      class="group flex items-center gap-1 rounded px-1 py-0.5 text-sm hover:bg-surface-2"
      :class="{ 'bg-surface-2 font-medium': selectedId === folder.id }"
    >
      <button
        type="button"
        class="w-4 text-muted hover:text-fg"
        :aria-label="expanded ? 'Collapse' : 'Expand'"
        data-test="folder-toggle"
        @click="expanded = !expanded"
      >
        {{ expanded ? '▾' : '▸' }}
      </button>

      <template v-if="renaming">
        <input
          v-model="renameValue"
          data-test="folder-rename-input"
          class="flex-1 rounded border border-border px-1 py-0.5 text-sm"
          @keyup.enter="submitRename"
          @keyup.esc="renaming = false"
        >
        <button
          type="button"
          data-test="folder-rename-submit"
          class="text-xs text-muted hover:text-fg"
          @click="submitRename"
        >
          Save
        </button>
      </template>

      <template v-else>
        <button
          type="button"
          data-test="folder-row"
          class="flex-1 truncate text-left"
          @click="emit('select', folder.id)"
        >
          {{ folder.name }}{{ folder.global ? ' (global)' : '' }}
        </button>
        <span class="hidden gap-1 text-xs text-muted group-hover:flex">
          <button
            type="button"
            data-test="folder-add-child"
            class="hover:text-fg"
            title="New subfolder"
            @click="addingChild = true"
          >
            +
          </button>
          <button
            type="button"
            data-test="folder-rename"
            class="hover:text-fg"
            title="Rename"
            @click="startRename"
          >
            ✎
          </button>
          <button
            type="button"
            data-test="folder-delete"
            class="hover:text-bad"
            title="Delete"
            @click="onDelete"
          >
            ✕
          </button>
        </span>
      </template>
    </div>

    <form
      v-if="addingChild"
      class="ml-5 mt-1 flex items-center gap-1"
      @submit.prevent="submitChild"
    >
      <input
        v-model="childName"
        placeholder="Subfolder name"
        data-test="folder-child-input"
        class="flex-1 rounded border border-border px-1 py-0.5 text-sm"
        @keyup.esc="addingChild = false"
      >
      <button
        type="submit"
        data-test="folder-child-submit"
        class="text-xs text-muted hover:text-fg"
      >
        Add
      </button>
    </form>

    <ul
      v-if="expanded"
      class="ml-4 border-l border-border pl-1"
    >
      <FolderTree
        v-for="child in folders.childrenOf(folder.id)"
        :key="child.id"
        :folder="child"
        :selected-id="selectedId"
        @select="emit('select', $event)"
        @deleted="emit('deleted', $event)"
      />
    </ul>
  </li>
</template>
