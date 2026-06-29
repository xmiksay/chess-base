// Shared domain & API types for the chess-base frontend.
//
// Backend payloads are snake_case (mirrored verbatim where they cross the wire);
// store-local view models are camelCase. Board/move maps stay string-keyed here
// (chessground's nominal `Key` type is applied at the Board boundary only).

// --- primitives -------------------------------------------------------------

/** Deployment mode reported by `/api/health`. */
export type Mode = 'local' | 'server'

/** Board side / orientation. */
export type Color = 'white' | 'black'

/** A board square in algebraic form (e.g. `e4`). */
export type Square = string

/** Legal-move map for chessground: from-square → reachable destinations. */
export type Dests = Map<Square, Square[]>

/** A board move as emitted by the board / applied to a position. */
export interface BoardMove {
  from: Square
  to: Square
  promotion?: string
}

// --- identity & auth --------------------------------------------------------

export interface Health {
  mode: Mode
}

export interface User {
  id: string
  is_admin: boolean
  username?: string
}

export interface AuthResponse {
  token: string
  user: User
}

// --- databases (collections) ------------------------------------------------

export type DatabaseKind = 'lichess' | 'chesscom' | 'master' | 'own'

export interface Database {
  id: number
  owner_id: string | null
  name: string
  kind: DatabaseKind
  index_depth: number | null
  global: boolean
}

// --- games & search ---------------------------------------------------------

/** A game header row (game list, header search, position-search hits). */
export interface GameRow {
  id: number
  white: string | null
  black: string | null
  result: string | null
  date: string | null
  eco: string | null
  white_elo: number | null
  black_elo: number | null
  event?: string | null
}

/** A single game including its PGN movetext for board playback. */
export interface GameDetail extends GameRow {
  pgn: string
}

/** Keyset-paginated game list page (`/api/games`). */
export interface GamesPage {
  games: GameRow[]
  next_cursor: number | null
}

/** Keyset-paginated header-search page (`/api/search/headers`). */
export interface HeaderPage {
  games: GameRow[]
  next_cursor: string | null
}

/** One move's aggregate stats at a position (`/api/search/tree`). */
export interface MoveStat {
  san: string
  count: number
  white: number
  draws: number
  black: number
}

/** Header-search form state (camelCase; mapped to snake_case params in lib). */
export interface HeaderQuery {
  player: string
  color: string
  event: string
  result: string
  eco: string
  dateFrom: string
  dateTo: string
}

// --- studies (move trees) ---------------------------------------------------

/**
 * A board annotation pinned to a node, mirroring the chessground shape model so
 * it renders straight to the board (`src/pgn_tree.rs::Shape`). `dest` absent is a
 * single-square highlight; present is an arrow `orig`→`dest`.
 */
export interface Shape {
  orig: string
  dest?: string | null
  brush: string
}

/** One node of a study move tree (`src/pgn_tree.rs`). `children[0]` is mainline. */
export interface MoveNode {
  id: number
  parent: number | null
  san: string | null
  comment: string | null
  nags: number[]
  /** Pinned board shapes (issue #61); absent/empty for pre-#61 trees. */
  shapes?: Shape[]
  children: number[]
}

export interface MoveTree {
  root: number
  nodes: MoveNode[]
}

export interface StudySummary {
  id: number
  database_id: number
  name: string
  global: boolean
  owner_id: string | null
}

export interface Study extends StudySummary {
  tree: MoveTree
}

/** Result of appending a move: the new node id plus the refreshed study. */
export interface AddMoveResult {
  new_node_id: number
  study: Study
}

/** Comment / NAG annotation patch for a node. */
export interface Annotation {
  comment?: string | null
  nag?: number | null
}

/** A linearized token stream for rendering a move tree (lib/moveTree). */
export type MoveToken =
  | {
      type: 'move'
      id: number
      san: string | null
      nags: number[]
      comment: string | null
      number: string | null
      depth: number
    }
  | { type: 'open'; depth: number }
  | { type: 'close'; depth: number }

// --- engines ----------------------------------------------------------------

export interface EngineConfig {
  name: string
  path: string
  runner?: string | null
  weights?: string | null
}

export interface EngineDefault {
  default: string | null
}

/** Engine score from the side-to-move's perspective. */
export interface Score {
  type: 'cp' | 'mate'
  value: number
}

/** One piece's path across a plan line: the moving piece (color-cased FEN char)
 * and the squares it visits, origin included (`{piece:'N', squares:['g1','f3','g5']}`). */
export interface Trajectory {
  piece: string
  squares: Square[]
}

/** A PV line enriched with per-piece trajectories for the Plans overlay; the
 * `planline` WS frame emitted alongside each PV-bearing `info` (`src/plans.rs`). */
export interface PlanLine {
  multipv: number
  depth: number | null
  score: Score | null
  pv: string[]
  trajectories: Trajectory[]
}

/** Engine-analysis WebSocket events (`src/server/engine_ws.rs`). */
export type EngineMessage =
  | { type: 'ready'; name: string }
  | {
      type: 'info'
      depth?: number | null
      seldepth?: number | null
      multipv?: number | null
      score?: Score | null
      nodes?: number | null
      nps?: number | null
      time_ms?: number | null
      pv?: string[]
    }
  // PlanLine enriches a PV with per-piece trajectories for the Plans overlay
  // (#60); the pin button (#61) converts them to `Shape[]`.
  | {
      type: 'planline'
      multipv?: number | null
      depth?: number | null
      score?: Score | null
      pv: string[]
      trajectories: Trajectory[]
    }
  | { type: 'bestmove'; best_move: string; ponder?: string | null }
  | { type: 'error'; message: string }

/** A reduced principal-variation line held in the engine store. */
export interface EngineLine {
  multipv: number
  depth: number | null
  seldepth: number | null
  score: Score | null
  nodes: number | null
  nps: number | null
  timeMs: number | null
  pv: string[]
}

/** Terminal bestmove with a monotonic `seq` so repeats stay observable. */
export interface BestMove {
  move: string
  ponder: string | null
  seq: number
}

// --- replay / viewer --------------------------------------------------------

/** Position reached by replaying a SAN line (lib/openingTree). */
export interface ReplayPosition {
  fen: string
  dests: Dests
  lastMove: [Square, Square] | null
  turnColor: Color
  plies: number
  ok: boolean
}

/** One position in a replayed game (lib/pgnViewer). `ply` 0 is the start. */
export interface ViewerPosition {
  ply: number
  san: string | null
  fen: string | undefined
  lastMove: [Square, Square] | null
}

// --- settings ---------------------------------------------------------------

/** Backend settings payload (`/api/settings`). */
export interface ApiSettings {
  theme?: string
  board_theme?: string
  piece_set?: string
  default_database_id?: number | null
}

// --- imports ----------------------------------------------------------------

export type ImportSource = 'lichess' | 'chesscom'

export interface ImportResult {
  imported: number
}

export type JobStatus = 'running' | 'success' | 'error'

export interface ImportJob {
  id: number
  kind: string
  label: string
  status: JobStatus
  imported: number
  error: string | null
}

export type ImportState = 'idle' | 'running' | 'error' | 'partial' | 'done'

export interface ImportSummary {
  total: number
  running: number
  succeeded: number
  failed: number
  imported: number
  state: ImportState
}
