<script setup>
import { onMounted } from 'vue'
import { RouterLink, RouterView } from 'vue-router'
import { useSettingsStore } from './stores/settings.js'

const settings = useSettingsStore()

const navLinks = [
  { to: '/', label: 'Analysis' },
  { to: '/collections', label: 'Collections' },
  { to: '/games', label: 'Games' },
  { to: '/search', label: 'Search' },
]

onMounted(() => {
  settings.load()
})
</script>

<template>
  <div class="min-h-screen bg-neutral-50 text-neutral-900">
    <header class="border-b border-neutral-200 px-6 py-4">
      <div class="flex items-center justify-between">
        <h1 class="text-xl font-semibold">
          chess-base
        </h1>
        <nav class="flex items-center gap-4 text-sm">
          <RouterLink
            v-for="link in navLinks"
            :key="link.to"
            :to="link.to"
            class="text-neutral-600 hover:underline"
            active-class="font-semibold text-neutral-900"
          >
            {{ link.label }}
          </RouterLink>
          <RouterLink
            to="/settings"
            class="text-neutral-600 hover:underline"
            active-class="font-semibold text-neutral-900"
          >
            Settings
          </RouterLink>
          <RouterLink
            to="/login"
            class="text-neutral-600 hover:underline"
            active-class="font-semibold text-neutral-900"
          >
            Sign in
          </RouterLink>
        </nav>
      </div>
      <p class="text-sm text-neutral-500">
        Collect, store, search and study chess games.
      </p>
    </header>

    <main>
      <RouterView />
    </main>
  </div>
</template>
