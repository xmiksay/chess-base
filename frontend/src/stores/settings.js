// Pinia store for per-user UI settings (issue #13): theme, board theme, piece
// set and default database. It mirrors the server's `/api/settings` into
// localStorage so the UI renders the last-known preferences instantly on load,
// then reconciles with the backend (the source of truth across devices).

import { defineStore } from 'pinia'
import { ref } from 'vue'
import { api } from '../api.js'

const STORAGE_KEY = 'chess-base:settings'

export const THEMES = ['system', 'light', 'dark']
export const BOARD_THEMES = ['brown', 'blue', 'green']
export const PIECE_SETS = ['cburnett']

const DEFAULTS = {
  theme: 'system',
  boardTheme: 'brown',
  pieceSet: 'cburnett',
  defaultDatabaseId: null,
}

/** Map the backend's snake_case payload into the store's camelCase shape. */
function fromApi(s) {
  return {
    theme: s.theme ?? DEFAULTS.theme,
    boardTheme: s.board_theme ?? DEFAULTS.boardTheme,
    pieceSet: s.piece_set ?? DEFAULTS.pieceSet,
    defaultDatabaseId: s.default_database_id ?? DEFAULTS.defaultDatabaseId,
  }
}

/** Map the store's shape back to the backend payload, dropping defaults/nulls. */
function toApi(s) {
  const out = {}
  if (s.theme) out.theme = s.theme
  if (s.boardTheme) out.board_theme = s.boardTheme
  if (s.pieceSet) out.piece_set = s.pieceSet
  if (s.defaultDatabaseId != null) out.default_database_id = s.defaultDatabaseId
  return out
}

/** Read the localStorage mirror (best-effort; corrupt/absent → defaults). */
function readMirror() {
  try {
    const raw = window.localStorage.getItem(STORAGE_KEY)
    return raw ? { ...DEFAULTS, ...JSON.parse(raw) } : { ...DEFAULTS }
  } catch {
    return { ...DEFAULTS }
  }
}

export const useSettingsStore = defineStore('settings', () => {
  const mirror = readMirror()
  const theme = ref(mirror.theme)
  const boardTheme = ref(mirror.boardTheme)
  const pieceSet = ref(mirror.pieceSet)
  const defaultDatabaseId = ref(mirror.defaultDatabaseId)
  const error = ref(null)

  function snapshot() {
    return {
      theme: theme.value,
      boardTheme: boardTheme.value,
      pieceSet: pieceSet.value,
      defaultDatabaseId: defaultDatabaseId.value,
    }
  }

  function apply(s) {
    theme.value = s.theme
    boardTheme.value = s.boardTheme
    pieceSet.value = s.pieceSet
    defaultDatabaseId.value = s.defaultDatabaseId
    persistMirror()
    applyTheme()
  }

  function persistMirror() {
    try {
      window.localStorage.setItem(STORAGE_KEY, JSON.stringify(snapshot()))
    } catch {
      // storage unavailable (private mode / quota); server stays the truth.
    }
  }

  /** Reflect the resolved color scheme onto the document for CSS to react to. */
  function applyTheme() {
    const root = document?.documentElement
    if (!root) return
    const resolved =
      theme.value === 'system'
        ? window.matchMedia?.('(prefers-color-scheme: dark)')?.matches
          ? 'dark'
          : 'light'
        : theme.value
    root.classList.toggle('dark', resolved === 'dark')
    root.dataset.theme = resolved
  }

  /** Hydrate from the server, overriding the mirror with the source of truth. */
  async function load() {
    applyTheme() // render the mirror's theme immediately
    try {
      apply(fromApi(await api.settings.get()))
      error.value = null
    } catch (e) {
      // Offline / unauthenticated: keep the mirror so the UI still works.
      error.value = String(e.message ?? e)
    }
  }

  /** Apply a partial change, mirror it locally for instant UI, then persist. */
  async function update(patch) {
    const next = { ...snapshot(), ...patch }
    apply(next) // optimistic: UI + mirror update before the round trip
    try {
      apply(fromApi(await api.settings.set(toApi(next))))
      error.value = null
    } catch (e) {
      error.value = String(e.message ?? e)
    }
  }

  return {
    theme,
    boardTheme,
    pieceSet,
    defaultDatabaseId,
    error,
    snapshot,
    load,
    update,
    applyTheme,
  }
})
