import {
  createRouter,
  createWebHistory,
  type RouteLocationNormalized,
  type RouteRecordRaw,
  type Router,
  type RouterHistory,
} from 'vue-router'
import { useAuthStore } from '../stores/auth'

// Lazy-load views so each surface is split into its own chunk. The analysis
// view (`/`) is the landing page. Games/studies/assistant take an optional id
// param (issue #212) so every selectable object is URL-addressable; the views
// hydrate from the param and mirror selection back via `router.replace`.
export const routes: RouteRecordRaw[] = [
  { path: '/', name: 'analysis', component: () => import('../views/AnalysisView.vue') },
  { path: '/collections', name: 'collections', component: () => import('../views/CollectionsView.vue') },
  { path: '/games/:id?', name: 'games', component: () => import('../views/GamesView.vue') },
  { path: '/studies/:id?', name: 'studies', component: () => import('../views/StudyView.vue') },
  { path: '/assistant/:sessionId?', name: 'assistant', component: () => import('../views/AssistantView.vue') },
  { path: '/import', name: 'import', component: () => import('../views/ImportView.vue') },
  { path: '/search', name: 'search', component: () => import('../views/SearchView.vue') },
  { path: '/settings', name: 'settings', component: () => import('../views/SettingsView.vue') },
  { path: '/login', name: 'login', component: () => import('../views/LoginView.vue') },
]

// Routes whose object-id deep link stays reachable logged-out (issue #213,
// ADR-0045): the backend serves a `public`-flagged game/study to an anonymous
// caller, so `/games/:id` and `/studies/:id` must pass the guard. The bare
// `/games`/`/studies` list surfaces (no id) still need a session — they call
// list endpoints the anonymous tier can't reach.
const ANONYMOUS_DEEP_LINK_ROUTES = new Set(['games', 'studies'])

// Decide where a navigation should land given the auth state. Pure so it can be
// unit-tested without the router. Returns a redirect target or null to proceed.
//   - server mode + no session → bounce everything but /login (and an
//     anonymous-readable deep link, #213) to /login.
//   - server mode + signed in, heading to /login → send home.
//   - local (or unknown) mode → never gate.
export function authRedirect(
  to: Pick<RouteLocationNormalized, 'name' | 'fullPath' | 'params'>,
  { needsAuth, isServerMode }: { needsAuth: boolean; isServerMode: boolean },
) {
  const anonymousDeepLink =
    typeof to.name === 'string' && ANONYMOUS_DEEP_LINK_ROUTES.has(to.name) && to.params?.id != null
  if (needsAuth && to.name !== 'login' && !anonymousDeepLink) {
    return { name: 'login', query: { redirect: to.fullPath } }
  }
  if (isServerMode && !needsAuth && to.name === 'login') {
    return { name: 'analysis' }
  }
  return null
}

// `history` is injectable so tests can pass a memory history; production uses
// HTML5 history (deep links are served by the server's index.html fallback).
export function createAppRouter(history: RouterHistory = createWebHistory()): Router {
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
