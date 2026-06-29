<script setup lang="ts">
// Collection (databases) browser (issue #67): list the caller's databases plus
// global (admin-managed) ones with an ownership badge, and create / rename /
// delete the writable ones. Global databases render read-only for non-admins.
import { onMounted, reactive, ref } from 'vue'
import { useCollectionsStore } from '../stores/collections'
import type { Database, DatabaseKind } from '../types'

// The four collection kinds the backend accepts (db::entities::databases::KINDS).
const KINDS: DatabaseKind[] = ['own', 'lichess', 'chesscom', 'master']

const store = useCollectionsStore()
const form = reactive<{ name: string; kind: DatabaseKind; global: boolean }>({
  name: '',
  kind: 'own',
  global: false,
})
const editingId = ref<number | null>(null)
const editName = ref('')

async function create() {
  if (!form.name.trim()) return
  await store.create(form.name.trim(), form.kind, form.global)
  Object.assign(form, { name: '', kind: 'own', global: false })
}

function startRename(db: Database) {
  editingId.value = db.id
  editName.value = db.name
}

async function commitRename(id: number) {
  const name = editName.value.trim()
  if (name) await store.rename(id, name)
  editingId.value = null
}

function cancelRename() {
  editingId.value = null
}

const remove = (db: Database) => store.remove(db.id)

// The store records failures on `store.error`; swallow the rejection here so an
// offline/unauthenticated load doesn't surface as an unhandled rejection.
onMounted(() => store.refresh().catch(() => {}))
</script>

<template>
  <div class="mx-auto max-w-5xl p-6">
    <h2 class="text-lg font-semibold">
      Collections
    </h2>
    <p class="mt-1 text-sm text-neutral-500">
      Your databases of games, plus the global (admin-managed) ones.
    </p>

    <p
      v-if="store.error"
      class="mt-3 text-sm text-red-600"
      data-test="error"
    >
      {{ store.error }}
    </p>

    <ul
      v-if="store.list.length"
      class="mt-4 divide-y divide-neutral-100"
    >
      <li
        v-for="db in store.list"
        :key="db.id"
        class="flex items-center gap-3 py-2 text-sm"
        data-test="db-row"
      >
        <template v-if="editingId === db.id">
          <input
            v-model="editName"
            class="rounded border border-neutral-300 px-2 py-1 text-sm"
            aria-label="New name"
            @keyup.enter="commitRename(db.id)"
            @keyup.esc="cancelRename"
          >
          <button
            class="text-neutral-800 hover:underline"
            data-test="save"
            @click="commitRename(db.id)"
          >
            Save
          </button>
          <button
            class="text-neutral-500 hover:underline"
            @click="cancelRename"
          >
            Cancel
          </button>
        </template>
        <template v-else>
          <span class="font-medium">{{ db.name }}</span>
          <span class="text-neutral-500">{{ db.kind }}</span>
          <span
            class="rounded px-1.5 py-0.5 text-xs"
            :class="db.global
              ? 'bg-amber-100 text-amber-800'
              : 'bg-neutral-100 text-neutral-600'"
            data-test="badge"
          >
            {{ db.global ? 'Global' : 'Mine' }}
          </span>
          <template v-if="store.canWrite(db)">
            <button
              class="ml-auto text-neutral-700 hover:underline"
              data-test="rename"
              @click="startRename(db)"
            >
              Rename
            </button>
            <button
              class="text-red-600 hover:underline"
              data-test="delete"
              @click="remove(db)"
            >
              Delete
            </button>
          </template>
          <span
            v-else
            class="ml-auto text-xs text-neutral-400"
            data-test="readonly"
          >
            Read-only
          </span>
        </template>
      </li>
    </ul>
    <p
      v-else
      class="mt-4 text-sm text-neutral-500"
    >
      No collections yet.
    </p>

    <form
      class="mt-6 flex flex-wrap items-center gap-2"
      data-test="create-form"
      @submit.prevent="create"
    >
      <input
        v-model="form.name"
        placeholder="New collection name"
        class="rounded border border-neutral-300 px-2 py-1 text-sm"
      >
      <select
        v-model="form.kind"
        aria-label="Kind"
        class="rounded border border-neutral-300 px-2 py-1 text-sm"
      >
        <option
          v-for="k in KINDS"
          :key="k"
          :value="k"
        >
          {{ k }}
        </option>
      </select>
      <label
        v-if="store.isAdmin"
        class="flex items-center gap-1 text-sm text-neutral-600"
      >
        <input
          v-model="form.global"
          type="checkbox"
          data-test="global"
        >
        Global
      </label>
      <button
        type="submit"
        class="rounded bg-neutral-800 px-3 py-1 text-sm text-white hover:bg-neutral-700"
      >
        Create
      </button>
    </form>
  </div>
</template>
