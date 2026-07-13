<script setup lang="ts">
// "Add line to study" dialog (issue #173): grafts the position explorer's
// current SAN line into a study — either an existing one (picked from the
// studies store) or a brand-new one (database + name + folder, mirroring the
// plain "create study" flow). Optionally attaches the explorer's stat for the
// line's final position ("N games, W/D/L") as a comment on the grafted leaf.
import { computed, onMounted, ref } from 'vue'
import { api } from '../api'
import { useStudiesStore } from '../stores/studies'
import { useFoldersStore } from '../stores/folders'
import { formatMoveStat } from '../lib/openingTree'
import type { Database, MoveStat, Study } from '../types'

const props = defineProps<{ sans: string[]; stat: MoveStat | null }>()
const emit = defineEmits<{ close: [] }>()

const studies = useStudiesStore()
const folders = useFoldersStore()

const mode = ref<'existing' | 'new'>('existing')
const studyId = ref<number | null>(null)
const databases = ref<Database[]>([])
const databaseId = ref<number | null>(null)
const name = ref('')
const folderId = ref<number | null>(null)
const includeComment = ref(true)
const saving = ref(false)
const error = ref<string | null>(null)
const result = ref<Study | null>(null)

const statComment = computed(() => (props.stat ? formatMoveStat(props.stat) : null))

const canSubmit = computed(() => {
  if (saving.value || props.sans.length === 0) return false
  if (mode.value === 'existing') return studyId.value != null
  return !!name.value.trim() && databaseId.value != null
})

onMounted(async () => {
  try {
    const [, dbs] = await Promise.all([
      studies.list.length ? Promise.resolve(studies.list) : studies.refresh(),
      api.databases.list(),
    ])
    databases.value = dbs
    databaseId.value = dbs[0]?.id ?? null
    if (!folders.list.length) await folders.refresh().catch(() => {})
    mode.value = studies.list.length ? 'existing' : 'new'
    studyId.value = studies.list[0]?.id ?? null
  } catch (e) {
    error.value = String((e as Error)?.message ?? e)
  }
})

async function onSubmit() {
  if (!canSubmit.value) return
  saving.value = true
  error.value = null
  try {
    const comment = includeComment.value && statComment.value ? statComment.value : undefined
    result.value = await studies.addLine(
      mode.value === 'existing'
        ? { sans: props.sans, study_id: studyId.value!, comment }
        : {
            sans: props.sans,
            database_id: databaseId.value!,
            name: name.value.trim(),
            folder_id: folderId.value,
            comment,
          },
    )
  } catch (e) {
    error.value = String((e as Error)?.message ?? e)
  } finally {
    saving.value = false
  }
}
</script>

<template>
  <div
    class="fixed inset-0 z-50 flex items-center justify-center bg-black/40 p-4"
    data-test="add-line-dialog"
    @click.self="emit('close')"
  >
    <div class="w-full max-w-sm rounded bg-surface p-5 shadow-lg">
      <header class="mb-4 flex items-center justify-between">
        <h3 class="text-base font-semibold">
          Add line to study
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

      <!-- Result view after a successful add. -->
      <div
        v-if="result"
        data-test="add-line-result"
      >
        <p class="text-sm">
          Added to <span class="font-medium">{{ result.name }}</span>.
        </p>
        <div class="mt-4 flex justify-end">
          <button
            type="button"
            class="rounded bg-fg px-3 py-1 text-sm text-surface hover:opacity-90"
            @click="emit('close')"
          >
            Close
          </button>
        </div>
      </div>

      <!-- Form. -->
      <form
        v-else
        class="flex flex-col gap-3"
        @submit.prevent="onSubmit"
      >
        <p class="font-mono text-xs text-muted">
          {{ sans.join(' ') }}
        </p>

        <div class="flex gap-4 text-sm">
          <label class="flex items-center gap-1">
            <input
              v-model="mode"
              type="radio"
              name="add-line-mode"
              value="existing"
              data-test="mode-existing"
              :disabled="studies.list.length === 0"
            >
            Existing study
          </label>
          <label class="flex items-center gap-1">
            <input
              v-model="mode"
              type="radio"
              name="add-line-mode"
              value="new"
              data-test="mode-new"
            >
            New study
          </label>
        </div>

        <label
          v-if="mode === 'existing'"
          class="flex flex-col gap-1 text-sm"
        >
          Study
          <select
            v-model="studyId"
            data-test="study"
            class="rounded border border-border px-2 py-1"
          >
            <option
              v-for="s in studies.list"
              :key="s.id"
              :value="s.id"
            >
              {{ s.name }}{{ s.global ? ' (global)' : '' }}
            </option>
          </select>
        </label>

        <template v-else>
          <label class="flex flex-col gap-1 text-sm">
            Database
            <select
              v-model="databaseId"
              data-test="database"
              class="rounded border border-border px-2 py-1"
            >
              <option
                v-for="d in databases"
                :key="d.id"
                :value="d.id"
              >
                {{ d.name }}{{ d.global ? ' (global)' : '' }}
              </option>
            </select>
          </label>

          <label class="flex flex-col gap-1 text-sm">
            Name
            <input
              v-model="name"
              data-test="name"
              class="rounded border border-border px-2 py-1"
            >
          </label>

          <label class="flex flex-col gap-1 text-sm">
            Folder
            <select
              v-model="folderId"
              data-test="folder"
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
        </template>

        <label
          v-if="statComment"
          class="flex items-center gap-2 text-xs text-muted"
        >
          <input
            v-model="includeComment"
            type="checkbox"
            data-test="include-comment"
          >
          Attach stats comment: “{{ statComment }}”
        </label>

        <p
          v-if="error"
          class="text-xs text-bad"
          data-test="add-line-error"
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
            data-test="submit"
            class="rounded bg-fg px-3 py-1 text-sm text-surface hover:opacity-90 disabled:opacity-50"
            :disabled="!canSubmit"
          >
            {{ saving ? 'Adding…' : 'Add line' }}
          </button>
        </div>
      </form>
    </div>
  </div>
</template>
