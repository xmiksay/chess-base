// Thin client for the chess-base backend JSON API.

import type {
  AddMoveResult,
  Annotation,
  ApiSettings,
  AuthResponse,
  Database,
  DatabaseKind,
  EngineConfig,
  EngineDefault,
  GameDetail,
  GamesPage,
  GameRow,
  Health,
  HeaderPage,
  ImportResult,
  ImportSource,
  MoveStat,
  Shape,
  Study,
  StudySummary,
  User,
} from './types'

// Server-mode session token. The browser also receives an HttpOnly `session`
// cookie on login, but same-origin requests attach the token as a Bearer header
// too so the client controls when it authenticates (and can drop it on logout).
const TOKEN_KEY = 'chess-base:token'

let authToken = readStoredToken()

function readStoredToken(): string | null {
  try {
    return window.localStorage.getItem(TOKEN_KEY)
  } catch {
    return null
  }
}

/** Set (or clear, with a falsy value) the token used to authenticate requests. */
export function setAuthToken(token: string | null): void {
  authToken = token || null
  try {
    if (authToken) window.localStorage.setItem(TOKEN_KEY, authToken)
    else window.localStorage.removeItem(TOKEN_KEY)
  } catch {
    // Storage unavailable (private mode / quota): the in-memory token still
    // authenticates this tab; it just won't survive a reload.
  }
}

export function getAuthToken(): string | null {
  return authToken
}

/** Merge the Bearer auth header into a (possibly empty) header set. */
function withAuth(headers: Record<string, string> = {}): Record<string, string> {
  return authToken ? { ...headers, Authorization: `Bearer ${authToken}` } : { ...headers }
}

async function getJson<T>(path: string): Promise<T> {
  const res = await fetch(path, { headers: withAuth() })
  if (!res.ok) throw new Error(`${path} → ${res.status}`)
  return res.json() as Promise<T>
}

// The search endpoints stream NDJSON (one JSON object per line); parse it into
// an array. Blank lines are skipped so a trailing newline is harmless.
async function getNdjson<T>(path: string): Promise<T[]> {
  const res = await fetch(path, { headers: withAuth() })
  if (!res.ok) {
    let detail = ''
    try {
      detail = ((await res.json()) as { error?: string })?.error ?? ''
    } catch {
      // non-JSON error body; the status is enough.
    }
    throw new Error(detail || `${path} → ${res.status}`)
  }
  const text = await res.text()
  return text
    .split('\n')
    .filter((line) => line.trim())
    .map((line) => JSON.parse(line) as T)
}

async function send<T>(method: string, path: string, body?: unknown): Promise<T> {
  const res = await fetch(path, {
    method,
    headers: withAuth(body === undefined ? {} : { 'Content-Type': 'application/json' }),
    body: body === undefined ? undefined : JSON.stringify(body),
  })
  if (!res.ok) {
    let detail = ''
    try {
      detail = ((await res.json()) as { error?: string })?.error ?? ''
    } catch {
      // non-JSON error body; the status is enough.
    }
    throw new Error(detail || `${path} → ${res.status}`)
  }
  return (res.status === 204 ? null : await res.json()) as T
}

export const api = {
  health: () => getJson<Health>('/api/health'),

  // Identity of the caller (issue #67): { id, is_admin } — drives whether
  // global (admin-managed) collections render writable.
  whoami: () => getJson<User>('/api/whoami'),

  // Server-mode auth (issue #71). `register`/`login` return { token, user };
  // the caller stores the token via setAuthToken. `logout` is 204 (no body).
  // These 400 in local mode (no login — the single user is the implicit admin).
  auth: {
    register: (username: string, password: string) =>
      send<AuthResponse>('POST', '/api/auth/register', { username, password }),
    login: (username: string, password: string) =>
      send<AuthResponse>('POST', '/api/auth/login', { username, password }),
    logout: () => send<null>('POST', '/api/auth/logout'),
  },

  // Engine registry (issue #53): persisted multi-engine config + default.
  engines: {
    list: () => getJson<EngineConfig[]>('/api/engines'),
    default: () => getJson<EngineDefault>('/api/engines/default'),
    upsert: (config: EngineConfig) => send<EngineConfig>('POST', '/api/engines', config),
    setDefault: (name: string) => send<EngineDefault>('PUT', '/api/engines/default', { name }),
    remove: (name: string) => send<null>('DELETE', `/api/engines/${encodeURIComponent(name)}`),
  },

  // Study lifecycle CRUD + PGN import/export (issue #9). `list` returns
  // summaries; `get` / `create` / `importPgn` / `rename` return the full move tree.
  // The node mutations (issue #8) all return the refreshed study so the editor
  // re-renders from one response; `addMove` wraps it as `{ new_node_id, study }`.
  studies: {
    list: () => getJson<StudySummary[]>('/api/studies'),
    get: (id: number) => getJson<Study>(`/api/studies/${id}`),
    create: (databaseId: number, name: string, global = false) =>
      send<Study>('POST', '/api/studies', { database_id: databaseId, name, global }),
    importPgn: (databaseId: number, name: string, pgn: string, global = false) =>
      send<Study>('POST', '/api/studies/import', { database_id: databaseId, name, pgn, global }),
    exportPgn: (id: number) =>
      getJson<{ pgn: string }>(`/api/studies/${id}/export`).then((r) => r.pgn),
    rename: (id: number, name: string) => send<Study>('PATCH', `/api/studies/${id}`, { name }),
    remove: (id: number) => send<null>('DELETE', `/api/studies/${id}`),
    // Append a SAN move under `fromNodeId` (a variation when it already has kids).
    addMove: (id: number, fromNodeId: number, san: string) =>
      send<AddMoveResult>('POST', `/api/studies/${id}/moves`, { from_node_id: fromNodeId, san }),
    annotate: (id: number, nodeId: number, { comment, nag }: Annotation = {}) =>
      send<Study>('POST', `/api/studies/${id}/nodes/${nodeId}/annotate`, { comment, nag }),
    // Pin board shapes (a plan) to a node; an empty list clears them (#61).
    setShapes: (id: number, nodeId: number, shapes: Shape[]) =>
      send<Study>('PUT', `/api/studies/${id}/nodes/${nodeId}/shapes`, { shapes }),
    promote: (id: number, nodeId: number) =>
      send<Study>('POST', `/api/studies/${id}/nodes/${nodeId}/promote`),
    reorder: (id: number, nodeId: number, index: number) =>
      send<Study>('POST', `/api/studies/${id}/nodes/${nodeId}/reorder`, { index }),
    deleteNode: (id: number, nodeId: number) =>
      send<Study>('DELETE', `/api/studies/${id}/nodes/${nodeId}`),
  },

  // Ownable databases (issue #6): collections to search/import into. `list`
  // returns the caller's databases plus global (admin-managed) ones; `global`
  // on create makes an admin-owned database (requires admin server-side).
  databases: {
    list: () => getJson<Database[]>('/api/databases'),
    get: (id: number) => getJson<Database>(`/api/databases/${id}`),
    create: (name: string, kind: DatabaseKind, global = false) =>
      send<Database>('POST', '/api/databases', { name, kind, global }),
    rename: (id: number, name: string) => send<Database>('PATCH', `/api/databases/${id}`, { name }),
    remove: (id: number) => send<null>('DELETE', `/api/databases/${id}`),
  },

  // Game list + single-game fetch (issue #68). `list` is keyset-paginated:
  // pass `{ after }` (the previous page's `next_cursor`) to fetch the next page.
  // `get` returns the full game including PGN movetext for board playback.
  games: {
    list: (databaseId: number, { after, limit }: { after?: number; limit?: number } = {}) => {
      const params = new URLSearchParams({ database_id: String(databaseId) })
      if (after != null) params.set('after', String(after))
      if (limit != null) params.set('limit', String(limit))
      return getJson<GamesPage>(`/api/games?${params}`)
    },
    get: (id: number) => getJson<GameDetail>(`/api/games/${id}`),
  },

  // Game search (issues #6/#7). Header/metadata search (`headers`) is keyset-
  // paginated and returns one JSON page `{ games, next_cursor }`; pass the
  // previous page's `next_cursor` as `cursor` to advance. Position search
  // (`tree`/`games`) takes a FEN and streams NDJSON rows. `headers` takes the
  // query params built by lib/headerQuery.toParams.
  search: {
    headers: (params: Record<string, string> = {}) =>
      getJson<HeaderPage>(`/api/search/headers?${new URLSearchParams(params).toString()}`),
    tree: (fen: string) => getNdjson<MoveStat>(`/api/search/tree?fen=${encodeURIComponent(fen)}`),
    games: (fen: string, limit?: number) =>
      getNdjson<GameRow>(
        `/api/search/games?fen=${encodeURIComponent(fen)}` + (limit ? `&limit=${limit}` : ''),
      ),
  },

  // Game import (issue #70): trigger a Lichess / Chess.com sync, or upload a
  // PGN file, into a target database. Both return `{ imported }` — the number of
  // games ingested this run. A blank `token` is omitted (Lichess only).
  import: {
    sync: (databaseId: number, source: ImportSource, username: string, token?: string) =>
      send<ImportResult>('POST', '/api/import/sync', {
        database_id: databaseId,
        source,
        username,
        ...(token ? { token } : {}),
      }),
    uploadPgn: (databaseId: number, pgn: string) =>
      send<ImportResult>('POST', '/api/import/pgn', { database_id: databaseId, pgn }),
  },

  // Per-user settings (issue #13): theme, board theme, default database.
  settings: {
    get: () => getJson<ApiSettings>('/api/settings'),
    set: (settings: ApiSettings) => send<ApiSettings>('PUT', '/api/settings', settings),
  },
}
