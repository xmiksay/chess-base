<script setup lang="ts">
import { computed, onMounted } from 'vue'
import { RouterLink, RouterView, useRouter } from 'vue-router'
import { useSettingsStore } from './stores/settings'
import { useAuthStore } from './stores/auth'

const settings = useSettingsStore()
const auth = useAuthStore()
const router = useRouter()

const navLinks = [
  { to: '/', label: 'Analysis' },
  { to: '/collections', label: 'Collections' },
  { to: '/games', label: 'Games' },
  { to: '/studies', label: 'Studies' },
  { to: '/assistant', label: 'Assistant' },
  { to: '/import', label: 'Import' },
  { to: '/search', label: 'Search' },
]

// Cycle the color scheme: system → light → dark → system. The store mirrors the
// choice to localStorage and toggles `.dark` on <html> for the tokens to react.
const THEME_ORDER = ['system', 'light', 'dark'] as const
const themeIcon = computed(
  () => ({ system: '🖥', light: '☀', dark: '🌙' })[settings.theme] ?? '🖥',
)
function cycleTheme() {
  const i = THEME_ORDER.indexOf(settings.theme as (typeof THEME_ORDER)[number])
  settings.update({ theme: THEME_ORDER[(i + 1) % THEME_ORDER.length] })
}

async function signOut() {
  await auth.logout()
  router.push('/login')
}

onMounted(() => {
  auth.init()
  settings.load()
})
</script>

<template>
  <div class="min-h-screen bg-surface-2 text-fg">
    <header class="sticky top-0 z-20 border-b border-border bg-surface shadow-sm">
      <div class="mx-auto flex max-w-6xl flex-col gap-1 px-6 py-3">
        <div class="flex items-center justify-between gap-4">
          <RouterLink
            to="/"
            class="text-xl font-bold tracking-tight"
          >
            chess-base
          </RouterLink>

          <nav class="flex flex-wrap items-center gap-1 text-sm">
            <RouterLink
              v-for="link in navLinks"
              :key="link.to"
              :to="link.to"
              class="rounded-md px-3 py-1.5 font-medium text-muted transition hover:bg-surface-2 hover:text-fg"
              active-class="bg-surface-2 text-fg"
            >
              {{ link.label }}
            </RouterLink>

            <span class="mx-1 h-5 w-px bg-border" />

            <button
              type="button"
              class="rounded-md px-2 py-1.5 text-base leading-none transition hover:bg-surface-2"
              :title="`Theme: ${settings.theme} (click to change)`"
              data-test="theme-toggle"
              @click="cycleTheme"
            >
              {{ themeIcon }}
            </button>

            <RouterLink
              to="/settings"
              class="rounded-md px-3 py-1.5 font-medium text-muted transition hover:bg-surface-2 hover:text-fg"
              active-class="bg-surface-2 text-fg"
            >
              Settings
            </RouterLink>

            <!-- Auth controls are server-mode only; local mode is implicit admin. -->
            <template v-if="auth.isServerMode">
              <span
                v-if="auth.user"
                class="px-2 text-muted"
              >{{ auth.user.id }}</span>
              <button
                v-if="auth.user"
                type="button"
                class="rounded-md px-3 py-1.5 font-medium text-muted transition hover:bg-surface-2 hover:text-fg"
                @click="signOut"
              >
                Sign out
              </button>
              <RouterLink
                v-else
                to="/login"
                class="rounded-md px-3 py-1.5 font-medium text-muted transition hover:bg-surface-2 hover:text-fg"
                active-class="bg-surface-2 text-fg"
              >
                Sign in
              </RouterLink>
            </template>
          </nav>
        </div>
        <p class="text-sm text-muted">
          Collect, store, search and study chess games.
        </p>
      </div>
    </header>

    <main>
      <RouterView />
    </main>
  </div>
</template>
