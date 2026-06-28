// Thin client for the chess-base backend JSON API.

async function getJson(path) {
  const res = await fetch(path)
  if (!res.ok) throw new Error(`${path} → ${res.status}`)
  return res.json()
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
}
