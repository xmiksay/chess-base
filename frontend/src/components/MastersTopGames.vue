<script setup lang="ts">
// Lichess Masters top games for the explorer's position (ADR-0053). The masters
// DB stays remote; "Import" pulls one game into a database the caller picks —
// the only way a masters game lands in the (slim) local DB.
import { computed, onMounted, ref } from 'vue'
import { api } from '../api'
import { useCollectionsStore } from '../stores/collections'
import type { MastersGame } from '../types'

defineProps<{ games: MastersGame[] }>()

const collections = useCollectionsStore()
const targets = computed(() => collections.list.filter((d) => collections.canWrite(d)))
const databaseId = ref<number | null>(null)
// Per-game outcome of the last import attempt, keyed by Lichess id.
const status = ref<Record<string, string>>({})
const busy = ref<string | null>(null)

function result(g: MastersGame): string {
  if (g.winner === 'white') return '1-0'
  if (g.winner === 'black') return '0-1'
  return '½-½'
}

function player(name: string, rating: number | null): string {
  return rating ? `${name} (${rating})` : name
}

async function importGame(g: MastersGame) {
  if (databaseId.value == null) return
  busy.value = g.id
  try {
    const r = await api.explorer.importMasters(g.id, databaseId.value)
    status.value[g.id] = r.imported > 0 ? 'Imported' : 'Already in database'
  } catch (e) {
    status.value[g.id] = String((e as Error)?.message ?? e)
  } finally {
    busy.value = null
  }
}

onMounted(async () => {
  try {
    await collections.refresh()
  } catch {
    // The store surfaces the error; the picker just stays empty.
  }
  databaseId.value = targets.value[0]?.id ?? null
})
</script>

<template>
  <div>
    <div class="mb-2 mt-6 flex flex-wrap items-center gap-2">
      <h3 class="text-sm font-semibold text-muted">
        Top games
      </h3>
      <label class="ml-auto flex items-center gap-1 text-xs text-muted">
        Import into
        <select
          v-model="databaseId"
          data-test="masters-import-target"
          class="rounded border border-border bg-surface px-2 py-1 text-sm"
        >
          <option
            v-if="targets.length === 0"
            :value="null"
          >
            no writable database
          </option>
          <option
            v-for="d in targets"
            :key="d.id"
            :value="d.id"
          >
            {{ d.name }}
          </option>
        </select>
      </label>
    </div>
    <p
      v-if="games.length === 0"
      class="text-sm text-muted"
    >
      No master games reach this position.
    </p>
    <ul class="flex flex-col gap-1">
      <li
        v-for="g in games"
        :key="g.id"
        data-test="masters-game-row"
        class="flex flex-wrap items-center gap-x-2 text-sm"
      >
        <a
          :href="`https://lichess.org/${g.id}`"
          target="_blank"
          rel="noopener"
          class="hover:underline"
        >
          <span class="font-medium">{{ player(g.white, g.white_rating) }}</span>
          <span class="text-muted"> vs </span>
          <span class="font-medium">{{ player(g.black, g.black_rating) }}</span>
        </a>
        <span class="font-mono text-muted">{{ result(g) }}</span>
        <span
          v-if="g.year"
          class="text-muted"
        >· {{ g.year }}</span>
        <button
          type="button"
          data-test="masters-import"
          class="ml-auto rounded border border-border px-2 py-0.5 text-xs disabled:opacity-50"
          :disabled="databaseId == null || busy === g.id"
          @click="importGame(g)"
        >
          {{ busy === g.id ? 'Importing…' : 'Import' }}
        </button>
        <span
          v-if="status[g.id]"
          data-test="masters-import-status"
          class="w-full text-xs text-muted"
        >{{ status[g.id] }}</span>
      </li>
    </ul>
  </div>
</template>
