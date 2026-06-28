import { createRouter, createWebHistory } from 'vue-router'

// Lazy-load views so each surface is split into its own chunk. The analysis
// view is the landing page; the rest are stubs filled in by follow-up issues.
export const routes = [
  { path: '/', name: 'analysis', component: () => import('../views/AnalysisView.vue') },
  { path: '/collections', name: 'collections', component: () => import('../views/CollectionsView.vue') },
  { path: '/games', name: 'games', component: () => import('../views/GamesView.vue') },
  { path: '/search', name: 'search', component: () => import('../views/SearchView.vue') },
  { path: '/settings', name: 'settings', component: () => import('../views/SettingsView.vue') },
  { path: '/login', name: 'login', component: () => import('../views/LoginView.vue') },
]

// `history` is injectable so tests can pass a memory history; production uses
// HTML5 history (deep links are served by the server's index.html fallback).
export function createAppRouter(history = createWebHistory()) {
  return createRouter({ history, routes })
}
