// Thin client for the chess-base backend JSON API.

async function getJson(path) {
  const res = await fetch(path)
  if (!res.ok) throw new Error(`${path} → ${res.status}`)
  return res.json()
}

// The search endpoints stream NDJSON (one JSON object per line); parse it into
// an array. Blank lines are skipped so a trailing newline is harmless.
async function getNdjson(path) {
  const res = await fetch(path)
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
    headers: body === undefined ? {} : { 'Content-Type': 'application/json' },
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
