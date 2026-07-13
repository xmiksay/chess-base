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
  /** Capability flags (issue #119): whether an engine / LLM is configured. */
  engine?: boolean
  llm?: boolean
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

/** Offset-paginated game list page (`/api/games`), with a total for the paginator. */
export interface GamesPage {
  games: GameRow[]
  total: number
  page: number
  limit: number
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

/** Position-explorer filter form state (camelCase; mapped to snake_case query
 * params). A subset of `HeaderQuery` — player/color/date range only — applied
 * to the opening-tree/games-list explorer and the study generator (issue #172).
 * Mirrors the backend `PositionFilter` (src/search/position.rs). */
export interface PositionFilter {
  player: string
  color: string
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
/** Engine evaluation pinned to a node (issue #120), always White's perspective.
 *  Mirrors the backend `Eval` enum: centipawns or a signed mate distance. */
export type Eval = { cp: number } | { mate: number }

export interface MoveNode {
  id: number
  parent: number | null
  san: string | null
  comment: string | null
  nags: number[]
  /** Pinned board shapes (issue #61); absent/empty for pre-#61 trees. */
  shapes?: Shape[]
  /** Engine evaluation after this move (issue #120); absent when unevaluated. */
  eval?: Eval
  children: number[]
}

export interface MoveTree {
  root: number
  nodes: MoveNode[]
  /** Set-up start position the moves replay from; absent ⇒ the standard start. */
  start_fen?: string
}

export interface StudySummary {
  id: number
  database_id: number
  name: string
  global: boolean
  owner_id: string | null
  /** Folder the study is filed under (issue #164); null ⇒ unfiled (root). */
  folder_id: number | null
  /** Game this study was saved as an analysis of (issue #164); null otherwise. */
  origin_game_id: number | null
}

/** A study folder in the hierarchy (issue #164). */
export interface FolderSummary {
  id: number
  owner_id: string | null
  parent_id: number | null
  name: string
  global: boolean
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

// --- game review (issue #119, Mode A) ---------------------------------------

/** Engine-only move quality buckets, worst → best (backend `MoveReview`). */
export type MoveClassification =
  | 'best'
  | 'great'
  | 'good'
  | 'inaccuracy'
  | 'mistake'
  | 'blunder'

/** One reviewed ply (`POST /api/games/{id}/analyse`). Eval is White's perspective. */
export interface MoveReview {
  ply: number // 1-based
  san: string // played move, normalized SAN
  eval_cp: number // centipawns, mate clamped to ±1000
  mate?: number // signed mate distance, White's perspective; omitted if not mate
  best_move?: string // engine's preferred move (SAN); omitted when played was best
  best_line?: string[] // engine PV from the position before (SAN, ≤6 plies); omitted when empty
  played_rank?: number // 1 = best; omitted when outside the top engine lines
  classification: MoveClassification
  explanation: string
}

/** Per-side review aggregates. */
export interface SideSummary {
  acpl: number
  accuracy: number
  inaccuracies: number
  mistakes: number
  blunders: number
}

export interface ReviewSummary {
  white: SideSummary
  black: SideSummary
}

/** Full engine-only game review. */
export interface GameReview {
  start_fen: string
  moves: MoveReview[]
  summary: ReviewSummary
}

// --- study generation (issue #119, Mode B) ----------------------------------

/** Tree-shaping knobs for LLM study generation; mirrors `src/study_gen/tree.rs`. */
export interface TreeConfig {
  max_depth?: number // default 6 (variation depth in plies)
  max_children?: number // default 3 (continuations per node)
  max_nodes?: number // default 64
  min_frequency?: number // default 0.05
  eval_margin_cp?: number // default 100
}

/** Request body for `POST /api/studies/generate`. */
export interface GenerateBody {
  database_id: number
  name: string
  global?: boolean
  start_fen?: string // defaults to startpos server-side
  model?: string
  engine_depth?: number // per-position search depth, capped server-side
  tree?: TreeConfig
  plan_lines?: number // pin top-N engine "plan" arrows per node (0–3, 0 = off)
  threats?: boolean // pin static hanging-piece "threat" arrows per node
  player?: string // restrict continuations to this player's games (either side unless color narrows it)
  color?: string // 'white' | 'black'; ignored without player
  date_from?: string // only games on/after this PGN date (inclusive)
  date_to?: string // only games on/before this PGN date (inclusive)
}

/** Result of a generation run. */
export interface GenerateView {
  id: number
  database_id: number
  name: string
  global: boolean
  node_count: number
  rejected: number
}

// --- danger-map study generation (issue #131, ADR-0026) ---------------------

/**
 * Spine walk + classifier knobs for the danger-map generator; partial overrides
 * over the server defaults. Mirrors `SpineConfig` in `src/study_gen/spine.rs`.
 */
export interface SpineConfig {
  our_side?: 'White' | 'Black' // which side the repertoire plays (default White)
  max_depth?: number // plies from the root (default 8)
  min_frequency?: number // drop replies rarer than this share (default 0.02)
  max_replies?: number // opponent replies expanded per position (default 4)
  min_miss_rate?: number // humans must miss the only move this often (default 0.3)
}

/** Request body for `POST /api/studies/generate-danger-map`. */
export interface DangerMapBody {
  database_id: number
  name: string
  spine_pgn: string // the repertoire spine as PGN movetext to walk for danger
  global?: boolean
  start_fen?: string // defaults to startpos server-side
  model?: string
  spine?: SpineConfig
  movetime_ms?: number // per-variation engine budget, capped server-side
  multipv?: number // MultiPV line count, floored at 2 server-side
}

/** One engine-adjudicated danger role surfaced on a danger-map result. */
export interface DangerMapRole {
  node_id: number
  san: string | null
  kind: string // Trap | OnlyMove | Attack | OffBook
  role: string // Weapon | Caution | OffBook
}

/** Result of a danger-map generation run. */
export interface DangerMapView {
  id: number
  database_id: number
  name: string
  global: boolean
  node_count: number
  rejected: number
  roles: DangerMapRole[]
}

// --- engine-only danger walk (issue #156, ADR-0026 / ADR-0027) --------------
// The lightweight, LLM-free sibling of the generator above: `POST
// /api/studies/danger-map` returns the raw engine-adjudicated tree for a spine so
// the SPA can render the danger overlay without any language model configured.

/** Trap-test verdict on a candidate move (`src/study_gen/danger.rs`). */
export type TrapVerdict = 'Weapon' | 'HopeChess' | 'Quiet'

/** A pawn storm toward our king found in the opponent's best line (issue #142). */
export interface AttackSignal {
  pawn: string // colour-cased FEN char of the storming pawn ('P' / 'p')
  path: string[] // the pawn's squares across the line, origin first
  advances: number // forward pushes along the path
}

/** The danger signal on one node, with the raw figures behind the verdict. */
export interface DangerTag {
  kind: string // Trap | OnlyMove | Attack | OffBook
  role: string // Weapon | Caution | OffBook
  trap?: TrapVerdict // trap verdict on the move that reached this node
  only_move_gap?: number // PV1 − PV2 gap (opponent's perspective), centipawns
  miss_rate?: number // share of DB games humans missed the best reply (0..1)
  attack?: AttackSignal
}

/** One node of the engine-adjudicated danger tree (`src/study_gen/spine.rs`). */
export interface DangerNode {
  id: number
  parent: number | null
  san?: string // move leading here; absent only at the root
  fen: string
  ply: number
  tag?: DangerTag // present only on flagged (dangerous) moves
  children: number[]
}

/** A walked, tagged repertoire tree — the output of the engine danger walk. */
export interface DangerTree {
  nodes: DangerNode[]
  root: number
}

/** Request body for `POST /api/studies/danger-map` (engine only, no LLM). */
export interface DangerWalkBody {
  spine_pgn: string // the repertoire spine as PGN movetext to walk for danger
  fen?: string // defaults to startpos server-side
  spine?: SpineConfig
  movetime_ms?: number // per-variation engine budget, capped server-side
  multipv?: number // MultiPV line count, floored at 2 server-side
}

/** Result of the engine danger walk: the full tree + a flat tagged-node digest. */
export interface DangerWalkResult {
  tree: DangerTree
  roles: DangerMapRole[]
}

// --- settings ---------------------------------------------------------------

/** Backend settings payload (`/api/settings`). */
export interface ApiSettings {
  theme?: string
  board_theme?: string
  piece_set?: string
  default_database_id?: number | null
  // Board-overlay layer toggles (issue #123). Absent ⇒ the store's defaults
  // apply (plans on, threats/master off).
  show_plans?: boolean
  show_threats?: boolean
  show_master_moves?: boolean
  // Persistent engine options (MultiPV / Threads / Hash MB). Absent ⇒ store
  // defaults (3 / 4 / 16).
  engine_multipv?: number
  engine_threads?: number
  engine_hash_mb?: number
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

// --- AI study assistant (issue #20) -----------------------------------------

/** A model-requested tool call; `requires_approval` marks a mutating tool. */
export interface AssistantToolCall {
  id: string
  name: string
  input: unknown
  requires_approval: boolean
}

/** A tool result fed back into the loop. */
export interface AssistantToolResult {
  tool_call_id: string
  content: string
  is_error: boolean
}

/** One transcript turn: `role` plus whichever of the optional fields applies. */
export interface AssistantMessage {
  role: 'user' | 'assistant' | 'tool_results'
  text?: string
  tool_calls?: AssistantToolCall[]
  tool_results?: AssistantToolResult[]
}

/** A session sidebar row (no transcript). */
export interface AssistantSessionSummary {
  id: number
  title: string
  model: string
}

/** A full session with its transcript + agent-loop state. */
export interface AssistantSession {
  id: number
  title: string
  model: string
  messages: AssistantMessage[]
  /** Mutating calls awaiting an approve/deny decision. */
  pending_approvals: AssistantToolCall[]
  awaiting_approval: boolean
  iterations: number
  iteration_cap: number
}
