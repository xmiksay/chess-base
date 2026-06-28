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
}
