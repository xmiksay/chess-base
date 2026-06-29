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
</script>

<template>
  <div class="flex flex-col gap-6">
    <section class="rounded border border-neutral-200 p-4">
      <h2 class="mb-3 text-lg font-semibold">
        Appearance
      </h2>

      <p
        v-if="settings.error"
        class="mb-3 text-sm text-red-600"
        data-test="error"
      >
        {{ settings.error }}
      </p>

      <div class="grid gap-4 sm:grid-cols-2">
        <label class="flex flex-col gap-1 text-sm">
          <span class="font-medium">Theme</span>
          <select
            class="rounded border border-neutral-300 px-2 py-1"
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
            class="rounded border border-neutral-300 px-2 py-1"
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
            class="rounded border border-neutral-300 px-2 py-1"
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
            class="rounded border border-neutral-300 px-2 py-1"
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

    <EnginesSettings />
  </div>
</template>
