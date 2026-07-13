// Thin client for the chess-base backend JSON API.

import type {
  AddMoveResult,
  Annotation,
  ApiSettings,
  AssistantSession,
  AssistantSessionSummary,
  AuthResponse,
  Database,
  DatabaseKind,
  EngineConfig,
  EngineDefault,
  FolderSummary,
  GameDetail,
  GameReview,
  GamesPage,
  GameRow,
  DangerMapBody,
  DangerMapView,
  DangerTree,
  DangerWalkBody,
  DangerWalkResult,
  GenerateBody,
  GenerateView,
  Health,
  HeaderPage,
  ImportResult,
  ImportSource,
  MoveStat,
  MoveTree,
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

// Fetch a plain-text body (the `.pgn` export downloads, issue #120).
async function getText(path: string): Promise<string> {
  const res = await fetch(path, { headers: withAuth() })
  if (!res.ok) throw new Error(`${path} → ${res.status}`)
  return res.text()
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
    // Download a study as a `.pgn` file (issue #120). `eval` (default true) keeps
    // the per-move `[%eval]` annotations; `false` exports plain movetext.
    exportPgn: (id: number, { eval: withEval = true }: { eval?: boolean } = {}) =>
      getText(`/api/studies/${id}/export?eval=${withEval}`),
    // Fill `[%eval]` on every non-terminal node from the engine (issue #162), so
    // the exported PGN carries evals Lichess renders. Eval-only: comments / NAGs /
    // shapes stay put. Returns the refreshed study; 503 when no engine is configured.
    analyse: (id: number, depth?: number) =>
      send<Study>('POST', `/api/studies/${id}/analyse`, depth == null ? {} : { depth }),
    rename: (id: number, name: string) => send<Study>('PATCH', `/api/studies/${id}`, { name }),
    remove: (id: number) => send<null>('DELETE', `/api/studies/${id}`),
    // File a study under a folder (issue #164); `folderId` null ⇒ unfile to root.
    setFolder: (id: number, folderId: number | null) =>
      send<Study>('PUT', `/api/studies/${id}/folder`, { folder_id: folderId }),
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
    // LLM study generation (issue #119): tree → annotate/verify → persisted study.
    // 503 when no LLM is configured.
    generate: (body: GenerateBody) => send<GenerateView>('POST', '/api/studies/generate', body),
    // Danger-map generation (issue #131, ADR-0026): walk a repertoire spine and
    // surface traps / only-move paths instead of an engine-best tree. 503 when no
    // engine or LLM is configured.
    generateDangerMap: (body: DangerMapBody) =>
      send<DangerMapView>('POST', '/api/studies/generate-danger-map', body),
    // Engine-only danger walk (issue #156): walk a spine and return the raw
    // DangerTree (Weapon / Caution / Off-book) — no LLM, so the danger overlay
    // works on a no-key install. 503 when no engine is configured.
    dangerMap: (body: DangerWalkBody) =>
      send<DangerWalkResult>('POST', '/api/studies/danger-map', body),
    // Graft a walked DangerTree into this study as variations (deduped), so the
    // dangerous lines live in the PGN instead of a throwaway list. `atNodeId`
    // null ⇒ graft from the root. Returns the refreshed study.
    mergeDanger: (id: number, tree: DangerTree, atNodeId: number | null = null) =>
      send<Study>('POST', `/api/studies/${id}/merge-danger`, { tree, at_node_id: atNodeId }),
    // Merge many games' mainlines into one repertoire study (issue #170): frequency
    // orders the continuations and pins per-branch stats. `studyId` set ⇒ graft into
    // that study; otherwise a new study is created (`name` required) from the start.
    mergeGames: (body: {
      game_ids: number[]
      study_id?: number
      name?: string
      folder_id?: number | null
    }) => send<Study>('POST', '/api/studies/merge-games', body),
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

  // Game list + single-game fetch (issue #68). `list` is offset-paginated and
  // sortable: pass `{ page }` (0-based), `{ limit }`, and `{ sort, dir }`
  // (default date / desc = newest-first). The page carries `total` for the
  // paginator. `get` returns the full game including PGN for board playback.
  games: {
    list: (
      databaseId: number,
      { page, limit, sort, dir }: { page?: number; limit?: number; sort?: string; dir?: string } = {},
    ) => {
      const params = new URLSearchParams({ database_id: String(databaseId) })
      if (page != null) params.set('page', String(page))
      if (limit != null) params.set('limit', String(limit))
      if (sort) params.set('sort', sort)
      if (dir) params.set('dir', dir)
      return getJson<GamesPage>(`/api/games?${params}`)
    },
    get: (id: number) => getJson<GameDetail>(`/api/games/${id}`),
    // Delete a game from its database (issue: collection CRUD). 204 on success;
    // 404 when the caller can't see / write the game's database.
    remove: (id: number) => send<null>('DELETE', `/api/games/${id}`),
    // The stored game as a variation tree (issue #135): the Rust PGN parser keeps
    // `(…)` sub-variations that the chess.js flattener drops. 422 (bad PGN), 404.
    tree: (id: number) => getJson<MoveTree>(`/api/games/${id}/tree`),
    // Fast engine-only full-game review (issue #119). `depth` omitted ⇒ backend
    // chooses. 503 (no engine), 422 (bad game), 404 (not found) → thrown Error.
    analyse: (id: number, depth?: number) =>
      send<GameReview>('POST', `/api/games/${id}/analyse` + (depth != null ? `?depth=${depth}` : '')),
    // Download a game as a `.pgn` file (issue #120). `annotated` runs the #119
    // review and embeds `[%eval]` + NAGs + why-notes (engine required, 503 else).
    exportPgn: (id: number, { annotated = false, depth }: { annotated?: boolean; depth?: number } = {}) => {
      const params = new URLSearchParams()
      if (annotated) params.set('annotated', 'true')
      if (depth != null) params.set('depth', String(depth))
      const qs = params.toString()
      return getText(`/api/games/${id}/export${qs ? `?${qs}` : ''}`)
    },
    // Persist a game as a study (issue #164): optionally filed under a folder and
    // optionally engine-analysed (503 when `analyse` is set but no engine).
    saveAsStudy: (
      id: number,
      body: { name: string; folder_id?: number | null; analyse?: boolean; depth?: number },
    ) => send<StudySummary>('POST', `/api/games/${id}/save-as-study`, body),
    // The analyses (studies) linked to a game (issue #164).
    linkedStudies: (id: number) => getJson<StudySummary[]>(`/api/games/${id}/studies`),
  },

  // Study folders (issue #164): a hierarchy to file studies under. `list` returns
  // the caller's folders plus global ones; `move` reparents (parentId null ⇒ root)
  // and requires the explicit `reparent` flag server-side so it isn't a rename.
  folders: {
    list: () => getJson<FolderSummary[]>('/api/folders'),
    create: (name: string, parentId: number | null = null, global = false) =>
      send<FolderSummary>('POST', '/api/folders', { name, parent_id: parentId, global }),
    rename: (id: number, name: string) => send<FolderSummary>('PATCH', `/api/folders/${id}`, { name }),
    move: (id: number, parentId: number | null) =>
      send<FolderSummary>('PATCH', `/api/folders/${id}`, { reparent: true, parent_id: parentId }),
    remove: (id: number) => send<null>('DELETE', `/api/folders/${id}`),
  },

  // Game search (issues #6/#7). Header/metadata search (`headers`) is keyset-
  // paginated and returns one JSON page `{ games, next_cursor }`; pass the
  // previous page's `next_cursor` as `cursor` to advance. Position search
  // (`tree`/`games`) takes a FEN and streams NDJSON rows; both also accept an
  // optional player/color/date filter (already-mapped snake_case params, issue
  // #172 — pass lib/positionFilter.toParams(filter)). `headers` takes the query
  // params built by lib/headerQuery.toParams.
  search: {
    headers: (params: Record<string, string> = {}) =>
      getJson<HeaderPage>(`/api/search/headers?${new URLSearchParams(params).toString()}`),
    tree: (fen: string, filterParams: Record<string, string> = {}) =>
      getNdjson<MoveStat>(`/api/search/tree?${new URLSearchParams({ fen, ...filterParams }).toString()}`),
    // Threatened-piece arrows for the side to move (issue #123, Threats overlay).
    threats: (fen: string) => getJson<Shape[]>(`/api/threats?fen=${encodeURIComponent(fen)}`),
    games: (fen: string, limit?: number, filterParams: Record<string, string> = {}) => {
      const params = new URLSearchParams({ fen, ...filterParams })
      if (limit) params.set('limit', String(limit))
      return getNdjson<GameRow>(`/api/search/games?${params.toString()}`)
    },
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

  // Embedded AI study assistant (issue #20). A chat session drives an agent loop
  // over the same tools the MCP endpoint exposes; mutating tools pause for the
  // user's approval. `create` / `send` / `respond` return the full session with
  // its transcript and loop state. 503 when no LLM provider is configured.
  assistant: {
    listSessions: () => getJson<AssistantSessionSummary[]>('/api/assistant/sessions'),
    getSession: (id: number) => getJson<AssistantSession>(`/api/assistant/sessions/${id}`),
    createSession: (title?: string, model?: string) =>
      send<AssistantSession>('POST', '/api/assistant/sessions', { title, model }),
    deleteSession: (id: number) => send<null>('DELETE', `/api/assistant/sessions/${id}`),
    // Post a user message and run the loop until it answers, pauses for an
    // approval, or hits the iteration cap.
    send: (id: number, text: string) =>
      send<AssistantSession>('POST', `/api/assistant/sessions/${id}/messages`, { text }),
    // Resolve a pending approval: a map of tool-call id → approve (true) / deny.
    respond: (id: number, decisions: Record<string, boolean>) =>
      send<AssistantSession>('POST', `/api/assistant/sessions/${id}/respond`, { decisions }),
  },

  // Per-user settings (issue #13): theme, board theme, default database.
  settings: {
    get: () => getJson<ApiSettings>('/api/settings'),
    set: (settings: ApiSettings) => send<ApiSettings>('PUT', '/api/settings', settings),
  },
}
