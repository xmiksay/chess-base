// Pinia store for server-mode authentication (issue #71): register / login /
// logout, the resolved caller, and the run mode. Local mode has no login — the
// single user is the implicit admin — so the store treats it as always
// authenticated and the UI hides the auth controls.
//
// The session token lives in `api.js` (attached as a Bearer header and mirrored
// to localStorage); this store owns the *user* and reconciles it with the server
// via `whoami` on startup so a reload doesn't flash the login screen.

import { defineStore } from 'pinia'
import { ref, computed } from 'vue'
import { api, setAuthToken, getAuthToken } from '../api'
import type { AuthResponse, Mode, User } from '../types'

const USER_KEY = 'chess-base:user'

/** Read the cached user (best-effort; corrupt/absent → null). */
function readUser(): User | null {
  try {
    const raw = window.localStorage.getItem(USER_KEY)
    return raw ? JSON.parse(raw) : null
  } catch {
    return null
  }
}

function persistUser(user: User | null) {
  try {
    if (user) window.localStorage.setItem(USER_KEY, JSON.stringify(user))
    else window.localStorage.removeItem(USER_KEY)
  } catch {
    // Storage unavailable; the in-memory user still drives this session.
  }
}

export const useAuthStore = defineStore('auth', () => {
  // null until the first `init()` resolves the deployment mode from /api/health.
  const mode = ref<Mode | null>(null)
  const user = ref<User | null>(readUser())
  const error = ref<string | null>(null)
  const ready = ref(false)

  const isServerMode = computed(() => mode.value === 'server')
  // Local (or unknown) mode never gates; server mode needs a resolved user.
  const isAuthenticated = computed(() => mode.value !== 'server' || user.value != null)
  // What the router guard keys on: a server-mode caller without a session.
  const needsAuth = computed(() => isServerMode.value && user.value == null)

  // init() runs once; concurrent callers (router guard + App mount) share it.
  let initPromise: Promise<void> | null = null

  function init(): Promise<void> {
    if (!initPromise) initPromise = resolve()
    return initPromise
  }

  async function resolve() {
    try {
      mode.value = (await api.health()).mode
    } catch {
      mode.value = null // offline: don't lock the user out of a local build.
    }
    if (mode.value === 'server' && getAuthToken()) {
      try {
        user.value = await api.whoami()
        persistUser(user.value)
      } catch {
        // Stale or invalid token: drop it so the UI shows the login form.
        setAuthToken(null)
        user.value = null
        persistUser(null)
      }
    } else if (mode.value === 'server' && !getAuthToken()) {
      // No token ⇒ no session, regardless of any cached user.
      user.value = null
      persistUser(null)
    }
    ready.value = true
  }

  async function register(username: string, password: string) {
    return authenticate(() => api.auth.register(username, password))
  }

  async function login(username: string, password: string) {
    return authenticate(() => api.auth.login(username, password))
  }

  /** Run an auth call, store its token + user, and report success as a boolean. */
  async function authenticate(call: () => Promise<AuthResponse>): Promise<boolean> {
    try {
      const { token, user: who } = await call()
      setAuthToken(token)
      user.value = who
      persistUser(who)
      error.value = null
      return true
    } catch (e) {
      // The backend sanitizes its messages (generic "internal error" for 5xx),
      // so surfacing e.message here leaks nothing internal.
      error.value = String((e as Error)?.message ?? e)
      return false
    }
  }

  async function logout() {
    try {
      await api.auth.logout()
    } catch {
      // Best-effort: a failed server call still clears the local session.
    }
    setAuthToken(null)
    user.value = null
    persistUser(null)
    error.value = null
  }

  return {
    mode,
    user,
    error,
    ready,
    isServerMode,
    isAuthenticated,
    needsAuth,
    init,
    register,
    login,
    logout,
  }
})
