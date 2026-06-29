<script setup>
// Game-import view (issue #70): pick a target database, then either sync from
// Lichess / Chess.com (username + optional token) or upload a `.pgn` file. Each
// import runs as a tracked job; the store folds their statuses into a summary.
import { computed, onMounted, reactive, ref } from 'vue'
import { useImportStore } from '../stores/import.js'

const store = useImportStore()

// The collection the import writes into; defaults to the first visible database.
const targetId = ref(null)

const sync = reactive({ source: 'lichess', username: '', token: '' })
const pgnFile = ref(null)

// Lichess accepts a personal token to raise rate limits; Chess.com is tokenless.
const supportsToken = computed(() => sync.source === 'lichess')
const canSync = computed(() => !!targetId.value && sync.username.trim().length > 0)
const canUpload = computed(() => !!targetId.value && !!pgnFile.value)

const summaryText = computed(() => {
  const s = store.summary
  if (s.state === 'running') return `Importing… ${s.running} running`
  const failed = s.failed ? ` — ${s.failed} failed` : ''
  return `${s.imported} game(s) imported across ${s.total} job(s)${failed}.`
})

function startSync() {
  if (!canSync.value) return
  store.syncSource({
    databaseId: targetId.value,
    source: sync.source,
    username: sync.username.trim(),
    token: supportsToken.value ? sync.token.trim() : '',
  })
}

function onFileChange(event) {
  pgnFile.value = event.target.files?.[0] ?? null
}

async function uploadPgn() {
  if (!canUpload.value) return
  const file = pgnFile.value
  const pgn = await file.text()
  store.uploadPgn({ databaseId: targetId.value, name: file.name, pgn })
}

onMounted(async () => {
  await store.loadDatabases()
  if (targetId.value == null && store.databases.length) {
    targetId.value = store.databases[0].id
  }
})
</script>

<template>
  <div class="mx-auto max-w-3xl p-6">
    <h2 class="text-lg font-semibold">
      Import games
    </h2>
    <p class="mt-1 text-sm text-neutral-500">
      Sync from Lichess or Chess.com, or upload a PGN file, into one of your
      collections.
    </p>

    <p
      v-if="store.error"
      class="mt-3 text-sm text-red-600"
      data-test="error"
    >
      {{ store.error }}
    </p>

    <label class="mt-4 flex flex-col gap-1 text-sm">
      <span class="font-medium">Target collection</span>
      <select
        v-model="targetId"
        aria-label="Target collection"
        class="rounded border border-neutral-300 px-2 py-1 text-sm"
        data-test="target"
      >
        <option
          v-for="db in store.databases"
          :key="db.id"
          :value="db.id"
        >
          {{ db.name }} ({{ db.kind }})
        </option>
      </select>
    </label>
    <p
      v-if="!store.databases.length"
      class="mt-2 text-sm text-neutral-500"
      data-test="no-databases"
    >
      No collections yet — create one first.
    </p>

    <!-- Provider sync -->
    <form
      class="mt-6 rounded border border-neutral-200 p-4"
      data-test="sync-form"
      @submit.prevent="startSync"
    >
      <h3 class="text-sm font-semibold">
        Sync from a provider
      </h3>
      <div class="mt-3 flex flex-wrap items-end gap-2">
        <label class="flex flex-col gap-1 text-sm">
          <span class="text-neutral-600">Source</span>
          <select
            v-model="sync.source"
            aria-label="Source"
            class="rounded border border-neutral-300 px-2 py-1 text-sm"
            data-test="source"
          >
            <option value="lichess">
              Lichess
            </option>
            <option value="chesscom">
              Chess.com
            </option>
          </select>
        </label>
        <label class="flex flex-col gap-1 text-sm">
          <span class="text-neutral-600">Username</span>
          <input
            v-model="sync.username"
            class="rounded border border-neutral-300 px-2 py-1 text-sm"
            placeholder="username"
            data-test="username"
          >
        </label>
        <label
          v-if="supportsToken"
          class="flex flex-col gap-1 text-sm"
        >
          <span class="text-neutral-600">API token (optional)</span>
          <input
            v-model="sync.token"
            class="rounded border border-neutral-300 px-2 py-1 text-sm"
            placeholder="lichess token"
            data-test="token"
          >
        </label>
        <button
          type="submit"
          :disabled="!canSync"
          class="rounded bg-emerald-600 px-3 py-1 text-sm font-medium text-white hover:bg-emerald-700 disabled:opacity-50"
          data-test="sync-submit"
        >
          Sync
        </button>
      </div>
    </form>

    <!-- PGN upload -->
    <form
      class="mt-4 rounded border border-neutral-200 p-4"
      data-test="pgn-form"
      @submit.prevent="uploadPgn"
    >
      <h3 class="text-sm font-semibold">
        Upload a PGN file
      </h3>
      <div class="mt-3 flex flex-wrap items-center gap-2">
        <input
          type="file"
          accept=".pgn"
          aria-label="PGN file"
          class="text-sm"
          data-test="pgn-file"
          @change="onFileChange"
        >
        <button
          type="submit"
          :disabled="!canUpload"
          class="rounded bg-emerald-600 px-3 py-1 text-sm font-medium text-white hover:bg-emerald-700 disabled:opacity-50"
          data-test="pgn-submit"
        >
          Upload
        </button>
      </div>
    </form>

    <!-- Status -->
    <section
      v-if="store.jobs.length"
      class="mt-6"
    >
      <p
        class="text-sm font-medium"
        data-test="summary"
      >
        {{ summaryText }}
      </p>
      <ul class="mt-3 divide-y divide-neutral-100">
        <li
          v-for="job in store.jobs"
          :key="job.id"
          class="flex items-center gap-3 py-2 text-sm"
          data-test="job"
        >
          <span
            class="rounded px-1.5 py-0.5 text-xs"
            :class="{
              'bg-neutral-100 text-neutral-600': job.status === 'running',
              'bg-emerald-100 text-emerald-800': job.status === 'success',
              'bg-red-100 text-red-800': job.status === 'error',
            }"
            data-test="job-status"
          >
            {{ job.status }}
          </span>
          <span class="font-medium">{{ job.label }}</span>
          <span
            v-if="job.status === 'success'"
            class="text-neutral-500"
            data-test="job-imported"
          >
            {{ job.imported }} game(s)
          </span>
          <span
            v-else-if="job.status === 'error'"
            class="text-red-600"
            data-test="job-error"
          >
            {{ job.error }}
          </span>
        </li>
      </ul>
    </section>
  </div>
</template>
