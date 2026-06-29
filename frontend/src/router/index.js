import { createRouter, createWebHistory } from 'vue-router'
import { useAuthStore } from '../stores/auth.js'

// Lazy-load views so each surface is split into its own chunk. The analysis
// view is the landing page; the rest are stubs filled in by follow-up issues.
export const routes = [
  { path: '/', name: 'analysis', component: () => import('../views/AnalysisView.vue') },
  { path: '/collections', name: 'collections', component: () => import('../views/CollectionsView.vue') },
  { path: '/games', name: 'games', component: () => import('../views/GamesView.vue') },
  { path: '/studies', name: 'studies', component: () => import('../views/StudyView.vue') },
  { path: '/import', name: 'import', component: () => import('../views/ImportView.vue') },
  { path: '/search', name: 'search', component: () => import('../views/SearchView.vue') },
  { path: '/settings', name: 'settings', component: () => import('../views/SettingsView.vue') },
  { path: '/login', name: 'login', component: () => import('../views/LoginView.vue') },
]

// Decide where a navigation should land given the auth state. Pure so it can be
// unit-tested without the router. Returns a redirect target or null to proceed.
//   - server mode + no session → bounce everything but /login to /login.
//   - server mode + signed in, heading to /login → send home.
//   - local (or unknown) mode → never gate.
export function authRedirect(to, { needsAuth, isServerMode }) {
  if (needsAuth && to.name !== 'login') {
    return { name: 'login', query: { redirect: to.fullPath } }
  }
  if (isServerMode && !needsAuth && to.name === 'login') {
    return { name: 'analysis' }
  }
  return null
}

// `history` is injectable so tests can pass a memory history; production uses
// HTML5 history (deep links are served by the server's index.html fallback).
export function createAppRouter(history = createWebHistory()) {
  const router = createRouter({ history, routes })

  // Gate server-mode views behind auth. `init()` is idempotent — it resolves the
  // run mode (and restores the session) once, then returns the cached result.
  router.beforeEach(async (to) => {
    const auth = useAuthStore()
    await auth.init()
    return authRedirect(to, { needsAuth: auth.needsAuth, isServerMode: auth.isServerMode }) ?? true
  })

  return router
}
