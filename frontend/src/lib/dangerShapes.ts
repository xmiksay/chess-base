// Pure: turn the engine-adjudicated danger tree (`POST /api/studies/danger-map`,
// issue #156) into board arrows for the dangerous opponent replies *available from
// a given position*, plus a flat panel digest with the raw figures behind each
// verdict. SAN → from/to resolution uses chess.js against the FEN, so this stays a
// pure, framework-free function that unit-tests in isolation (mirrors masterShapes).

import { Chess } from 'chess.js'
import type { DrawShape, DrawBrush } from 'chessground/draw'
import type { AttackSignal, DangerTree, Eval, TrapVerdict } from '../types'

const WEAPON_COLOR = '#15803d' // green-700  — a prepared, recommendable line
const CAUTION_COLOR = '#dc2626' // red-600    — baits, but the best reply refutes it
const OFFBOOK_COLOR = '#d97706' // amber-600  — a move order the spine doesn't answer

/** chessground brush key for a danger role. Off-book / unknown roles fall to amber. */
export function brushForRole(role: string): string {
  if (role === 'Weapon') return 'dangerWeapon'
  if (role === 'Caution') return 'dangerCaution'
  return 'dangerOffbook'
}

/**
 * Brush table for the danger overlay: green Weapon, red Caution, amber Off-book.
 * Spread alongside the other overlay brushes into a board's `drawable.brushes`.
 */
export function dangerBrushes(): Record<string, DrawBrush> {
  return {
    dangerWeapon: { key: 'dgw', color: WEAPON_COLOR, opacity: 0.9, lineWidth: 10 },
    dangerCaution: { key: 'dgc', color: CAUTION_COLOR, opacity: 0.9, lineWidth: 10 },
    dangerOffbook: { key: 'dgo', color: OFFBOOK_COLOR, opacity: 0.85, lineWidth: 10 },
  }
}

/** Board-identity (first four) FEN fields — placement, side, castling, en-passant. */
function fenKey(fen: string): string {
  return String(fen)
    .trim()
    .split(/\s+/)
    .slice(0, 4)
    .join(' ')
}

/**
 * Danger arrows available *from* `fen`: for every flagged node whose parent sits at
 * `fen`, resolve its SAN to an arrow brushed by the node's role. SANs illegal in
 * `fen` (stale data) are skipped; an unparseable FEN yields no shapes.
 */
export function dangerShapesForFen(tree: DangerTree | null, fen: string): DrawShape[] {
  if (!tree || !Array.isArray(tree.nodes)) return []
  const key = fenKey(fen)
  let chess: Chess
  try {
    chess = new Chess(fen)
  } catch {
    return []
  }
  const moves = chess.moves({ verbose: true })
  const shapes: DrawShape[] = []
  for (const node of tree.nodes) {
    if (!node.tag || node.parent == null || !node.san) continue
    const parent = tree.nodes[node.parent]
    if (!parent || fenKey(parent.fen) !== key) continue
    const mv = moves.find((m) => m.san === node.san)
    if (!mv) continue
    shapes.push({
      orig: mv.from as DrawShape['orig'],
      dest: mv.to as DrawShape['dest'],
      brush: brushForRole(node.tag.role),
    })
  }
  return shapes
}

/** SAN line from the root down to `node` (the moves that reach this danger node). */
function lineSan(tree: DangerTree, node: DangerTree['nodes'][number]): string[] {
  const sans: string[] = []
  let cur: DangerTree['nodes'][number] | undefined = node
  while (cur && cur.parent != null) {
    if (cur.san) sans.push(cur.san)
    cur = tree.nodes[cur.parent]
  }
  return sans.reverse()
}

/** Move-number label for a ply, e.g. `3.dxc4` (White) or `3…dxc4` (Black). */
function moveLabel(ply: number, san: string): string {
  const moveNo = Math.ceil(ply / 2)
  return ply % 2 === 1 ? `${moveNo}.${san}` : `${moveNo}…${san}`
}

/** Format an `Eval` as a signed pawn score (`+0.30`) or mate count (`M3`). */
export function formatEval(evalScore: Eval): string {
  if ('mate' in evalScore) return evalScore.mate >= 0 ? `M${evalScore.mate}` : `-M${-evalScore.mate}`
  const pawns = evalScore.cp / 100
  return pawns >= 0 ? `+${pawns.toFixed(2)}` : pawns.toFixed(2)
}

/** One tagged node flattened for the side panel, with the raw figures it carries. */
export interface DangerRoleRow {
  nodeId: number
  san: string | null
  // The full line reaching this node and a move-numbered label for it, so the
  // panel can disambiguate the same move appearing in several lines (e.g. four
  // different "dxc4"s) and navigate the study tree to it.
  line: string[]
  label: string
  kind: string // Trap | OnlyMove | Attack | OffBook
  role: string // Weapon | Caution | OffBook
  onlyMoveGap: number | null // PV1 − PV2 gap, centipawns
  missRate: number | null // share of DB games humans missed the best reply (0..1)
  trap: TrapVerdict | null
  attack: AttackSignal | null
  eval: Eval | null // White-perspective eval of this node's position (issue #177)
}

/**
 * Flatten the tree's flagged nodes into panel rows (walk order = shallow, most
 * dangerous lines first), surfacing the raw figures behind each verdict so the
 * panel can quote them.
 */
export function dangerRoles(tree: DangerTree | null): DangerRoleRow[] {
  if (!tree || !Array.isArray(tree.nodes)) return []
  return tree.nodes
    .filter((n) => n.tag)
    .map((n) => {
      const tag = n.tag!
      return {
        nodeId: n.id,
        san: n.san ?? null,
        line: lineSan(tree, n),
        label: n.san ? moveLabel(n.ply, n.san) : '—',
        kind: tag.kind,
        role: tag.role,
        onlyMoveGap: tag.only_move_gap ?? null,
        missRate: tag.miss_rate ?? null,
        trap: tag.trap ?? null,
        attack: tag.attack ?? null,
        eval: tag.eval ?? null,
      }
    })
}
