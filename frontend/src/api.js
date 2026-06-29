// Thin client for the chess-base backend JSON API.

// Server-mode session token. The browser also receives an HttpOnly `session`
// cookie on login, but same-origin requests attach the token as a Bearer header
// too so the client controls when it authenticates (and can drop it on logout).
const TOKEN_KEY = 'chess-base:token'

let authToken = readStoredToken()

function readStoredToken() {
  try {
    return window.localStorage.getItem(TOKEN_KEY)
  } catch {
    return null
  }
}

/** Set (or clear, with a falsy value) the token used to authenticate requests. */
export function setAuthToken(token) {
  authToken = token || null
  try {
    if (authToken) window.localStorage.setItem(TOKEN_KEY, authToken)
    else window.localStorage.removeItem(TOKEN_KEY)
  } catch {
    // Storage unavailable (private mode / quota): the in-memory token still
    // authenticates this tab; it just won't survive a reload.
  }
}

export function getAuthToken() {
  return authToken
}

/** Merge the Bearer auth header into a (possibly empty) header set. */
function withAuth(headers = {}) {
  return authToken ? { ...headers, Authorization: `Bearer ${authToken}` } : { ...headers }
}

async function getJson(path) {
  const res = await fetch(path, { headers: withAuth() })
  if (!res.ok) throw new Error(`${path} → ${res.status}`)
  return res.json()
}

// The search endpoints stream NDJSON (one JSON object per line); parse it into
// an array. Blank lines are skipped so a trailing newline is harmless.
async function getNdjson(path) {
  const res = await fetch(path, { headers: withAuth() })
  if (!res.ok) {
    let detail = ''
    try {
      detail = (await res.json())?.error ?? ''
    } catch {
      // non-JSON error body; the status is enough.
    }
    throw new Error(detail || `${path} → ${res.status}`)
  }
  const text = await res.text()
  return text
    .split('\n')
    .filter((line) => line.trim())
    .map((line) => JSON.parse(line))
}

async function send(method, path, body) {
  const res = await fetch(path, {
    method,
    headers: withAuth(body === undefined ? {} : { 'Content-Type': 'application/json' }),
    body: body === undefined ? undefined : JSON.stringify(body),
  })
  if (!res.ok) {
    let detail = ''
    try {
      detail = (await res.json())?.error ?? ''
    } catch {
      // non-JSON error body; the status is enough.
    }
    throw new Error(detail || `${path} → ${res.status}`)
  }
  return res.status === 204 ? null : res.json()
}

export const api = {
  health: () => getJson('/api/health'),

  // Identity of the caller (issue #67): { id, is_admin } — drives whether
  // global (admin-managed) collections render writable.
  whoami: () => getJson('/api/whoami'),

  // Server-mode auth (issue #71). `register`/`login` return { token, user };
  // the caller stores the token via setAuthToken. `logout` is 204 (no body).
  // These 400 in local mode (no login — the single user is the implicit admin).
  auth: {
    register: (username, password) =>
      send('POST', '/api/auth/register', { username, password }),
    login: (username, password) => send('POST', '/api/auth/login', { username, password }),
    logout: () => send('POST', '/api/auth/logout'),
  },

  // Engine registry (issue #53): persisted multi-engine config + default.
  engines: {
    list: () => getJson('/api/engines'),
    default: () => getJson('/api/engines/default'),
    upsert: (config) => send('POST', '/api/engines', config),
    setDefault: (name) => send('PUT', '/api/engines/default', { name }),
    remove: (name) => send('DELETE', `/api/engines/${encodeURIComponent(name)}`),
  },

  // Study lifecycle CRUD + PGN import/export (issue #9). `list` returns
  // summaries; `get` / `create` / `importPgn` / `rename` return the full move tree.
  // The node mutations (issue #8) all return the refreshed study so the editor
  // re-renders from one response; `addMove` wraps it as `{ new_node_id, study }`.
  studies: {
    list: () => getJson('/api/studies'),
    get: (id) => getJson(`/api/studies/${id}`),
    create: (databaseId, name, global = false) =>
      send('POST', '/api/studies', { database_id: databaseId, name, global }),
    importPgn: (databaseId, name, pgn, global = false) =>
      send('POST', '/api/studies/import', { database_id: databaseId, name, pgn, global }),
    exportPgn: (id) => getJson(`/api/studies/${id}/export`).then((r) => r.pgn),
    rename: (id, name) => send('PATCH', `/api/studies/${id}`, { name }),
    remove: (id) => send('DELETE', `/api/studies/${id}`),
    // Append a SAN move under `fromNodeId` (a variation when it already has kids).
    addMove: (id, fromNodeId, san) =>
      send('POST', `/api/studies/${id}/moves`, { from_node_id: fromNodeId, san }),
    annotate: (id, nodeId, { comment, nag } = {}) =>
      send('POST', `/api/studies/${id}/nodes/${nodeId}/annotate`, { comment, nag }),
    promote: (id, nodeId) => send('POST', `/api/studies/${id}/nodes/${nodeId}/promote`),
    reorder: (id, nodeId, index) =>
      send('POST', `/api/studies/${id}/nodes/${nodeId}/reorder`, { index }),
    deleteNode: (id, nodeId) => send('DELETE', `/api/studies/${id}/nodes/${nodeId}`),
  },

  // Ownable databases (issue #6): collections to search/import into. `list`
  // returns the caller's databases plus global (admin-managed) ones; `global`
  // on create makes an admin-owned database (requires admin server-side).
  databases: {
    list: () => getJson('/api/databases'),
    get: (id) => getJson(`/api/databases/${id}`),
    create: (name, kind, global = false) =>
      send('POST', '/api/databases', { name, kind, global }),
    rename: (id, name) => send('PATCH', `/api/databases/${id}`, { name }),
    remove: (id) => send('DELETE', `/api/databases/${id}`),
  },

  // Game list + single-game fetch (issue #68). `list` is keyset-paginated:
  // pass `{ after }` (the previous page's `next_cursor`) to fetch the next page.
  // `get` returns the full game including PGN movetext for board playback.
  games: {
    list: (databaseId, { after, limit } = {}) => {
      const params = new URLSearchParams({ database_id: String(databaseId) })
      if (after != null) params.set('after', String(after))
      if (limit != null) params.set('limit', String(limit))
      return getJson(`/api/games?${params}`)
    },
    get: (id) => getJson(`/api/games/${id}`),
  },

  // Game search (issues #6/#7). Header/metadata search (`headers`) is keyset-
  // paginated and returns one JSON page `{ games, next_cursor }`; pass the
  // previous page's `next_cursor` as `cursor` to advance. Position search
  // (`tree`/`games`) takes a FEN and streams NDJSON rows. `headers` takes the
  // query params built by lib/headerQuery.toParams.
  search: {
    headers: (params = {}) =>
      getJson(`/api/search/headers?${new URLSearchParams(params).toString()}`),
    tree: (fen) => getNdjson(`/api/search/tree?fen=${encodeURIComponent(fen)}`),
    games: (fen, limit) =>
      getNdjson(
        `/api/search/games?fen=${encodeURIComponent(fen)}` +
          (limit ? `&limit=${limit}` : ''),
      ),
  },

  // Per-user settings (issue #13): theme, board theme, default database.
  settings: {
    get: () => getJson('/api/settings'),
    set: (settings) => send('PUT', '/api/settings', settings),
  },
}
