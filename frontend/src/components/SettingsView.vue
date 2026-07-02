<script setup lang="ts">
import { onMounted, ref } from 'vue'
import { api } from '../api'
import {
  useSettingsStore,
  THEMES,
  BOARD_THEMES,
  PIECE_SETS,
} from '../stores/settings'
import type { Database } from '../types'
import EnginesSettings from './EnginesSettings.vue'

// Per-user settings (issue #13): theme, board theme, piece set and default
// database. Each control writes through the store, which mirrors to localStorage
// for instant UI and persists to the backend.
const settings = useSettingsStore()
const databases = ref<Database[]>([])

const themes = THEMES
const boardThemes = BOARD_THEMES
const pieceSets = PIECE_SETS

onMounted(async () => {
  await settings.load()
  try {
    databases.value = await api.databases.list()
  } catch {
    // Listing failed (offline / unauthenticated); the selector just stays empty.
  }
})

const setTheme = (e: Event) =>
  settings.update({ theme: (e.target as HTMLSelectElement).value })
const setBoardTheme = (e: Event) =>
  settings.update({ boardTheme: (e.target as HTMLSelectElement).value })
const setPieceSet = (e: Event) =>
  settings.update({ pieceSet: (e.target as HTMLSelectElement).value })
const setDefaultDatabase = (e: Event) =>
  settings.update({
    defaultDatabaseId: (e.target as HTMLSelectElement).value
      ? Number((e.target as HTMLSelectElement).value)
      : null,
  })

// Persistent engine options, applied on every analysis board (issue: engine
// defaults). Clamped to the same ranges the backend validates.
const setEngineMultipv = (e: Event) =>
  settings.update({ engineMultipv: Number((e.target as HTMLSelectElement).value) })
const setEngineThreads = (e: Event) =>
  settings.update({ engineThreads: Number((e.target as HTMLInputElement).value) })
const setEngineHash = (e: Event) =>
  settings.update({ engineHash: Number((e.target as HTMLInputElement).value) })
</script>

<template>
  <div class="flex flex-col gap-6">
    <section class="rounded border border-border p-4">
      <h2 class="mb-3 text-lg font-semibold">
        Appearance
      </h2>

      <p
        v-if="settings.error"
        class="mb-3 text-sm text-bad"
        data-test="error"
      >
        {{ settings.error }}
      </p>

      <div class="grid gap-4 sm:grid-cols-2">
        <label class="flex flex-col gap-1 text-sm">
          <span class="font-medium">Theme</span>
          <select
            class="rounded border border-border px-2 py-1"
            data-test="theme"
            :value="settings.theme"
            @change="setTheme"
          >
            <option
              v-for="t in themes"
              :key="t"
              :value="t"
            >
              {{ t }}
            </option>
          </select>
        </label>

        <label class="flex flex-col gap-1 text-sm">
          <span class="font-medium">Board theme</span>
          <select
            class="rounded border border-border px-2 py-1"
            data-test="board-theme"
            :value="settings.boardTheme"
            @change="setBoardTheme"
          >
            <option
              v-for="t in boardThemes"
              :key="t"
              :value="t"
            >
              {{ t }}
            </option>
          </select>
        </label>

        <label class="flex flex-col gap-1 text-sm">
          <span class="font-medium">Piece set</span>
          <select
            class="rounded border border-border px-2 py-1"
            data-test="piece-set"
            :value="settings.pieceSet"
            @change="setPieceSet"
          >
            <option
              v-for="p in pieceSets"
              :key="p"
              :value="p"
            >
              {{ p }}
            </option>
          </select>
        </label>

        <label class="flex flex-col gap-1 text-sm">
          <span class="font-medium">Default database</span>
          <select
            class="rounded border border-border px-2 py-1"
            data-test="default-database"
            :value="settings.defaultDatabaseId ?? ''"
            @change="setDefaultDatabase"
          >
            <option value="">
              None
            </option>
            <option
              v-for="d in databases"
              :key="d.id"
              :value="d.id"
            >
              {{ d.name }}
            </option>
          </select>
        </label>
      </div>
    </section>

    <section class="rounded border border-border p-4">
      <h2 class="mb-1 text-lg font-semibold">
        Engine analysis
      </h2>
      <p class="mb-3 text-sm text-muted">
        Saved per user and used on every analysis board.
      </p>

      <div class="grid gap-4 sm:grid-cols-3">
        <label class="flex flex-col gap-1 text-sm">
          <span class="font-medium">Lines (MultiPV)</span>
          <select
            class="rounded border border-border bg-surface px-2 py-1"
            data-test="engine-multipv"
            :value="settings.engineMultipv"
            @change="setEngineMultipv"
          >
            <option
              v-for="n in 5"
              :key="n"
              :value="n"
            >
              {{ n }}
            </option>
          </select>
        </label>

        <label class="flex flex-col gap-1 text-sm">
          <span class="font-medium">Threads</span>
          <input
            type="number"
            min="1"
            max="64"
            class="rounded border border-border bg-surface px-2 py-1"
            data-test="engine-threads"
            :value="settings.engineThreads"
            @change="setEngineThreads"
          >
        </label>

        <label class="flex flex-col gap-1 text-sm">
          <span class="font-medium">Hash (MB)</span>
          <input
            type="number"
            min="1"
            max="4096"
            class="rounded border border-border bg-surface px-2 py-1"
            data-test="engine-hash"
            :value="settings.engineHash"
            @change="setEngineHash"
          >
        </label>
      </div>
    </section>

    <EnginesSettings />
  </div>
</template>
