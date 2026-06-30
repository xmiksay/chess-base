// Pure: turn database/master move statistics (`/api/search/tree`) into board
// arrows for the most-played continuations at a position (issue #123, Database
// moves overlay). Arrow thickness and a percentage label scale with each move's
// share of games. SAN → from/to resolution uses chess.js against the FEN, so
// this stays a pure, framework-free function that unit-tests in isolation.

import { Chess } from 'chess.js'
import type { DrawShape } from 'chessground/draw'
import type { MoveStat } from '../types'

/** Max master arrows drawn; more would clutter the board. */
export const MAX_MASTER_ARROWS = 5

const THIN = 6
const MEDIUM = 9
const THICK = 13

/** Arrow line width by a move's frequency (share of games at the position). */
function widthForFrequency(freq: number): number {
  if (freq >= 25) return THICK
  if (freq >= 10) return MEDIUM
  return THIN
}

/**
 * Build chessground arrows for the top continuations in `stats` at `fen`. Each
 * arrow runs from the move's origin to its destination, brushed `master`, sized
 * by frequency and labelled with that percentage. Rows whose SAN is illegal in
 * `fen` (stale data) are skipped; an unparseable FEN yields no shapes.
 */
export function masterMovesToShapes(fen: string, stats: MoveStat[]): DrawShape[] {
  if (!Array.isArray(stats) || stats.length === 0) return []
  let chess: Chess
  try {
    chess = new Chess(fen)
  } catch {
    return []
  }
  const moves = chess.moves({ verbose: true })
  const total = stats.reduce((sum, s) => sum + (s.count ?? 0), 0)
  if (total === 0) return []

  const top = [...stats].sort((a, b) => (b.count ?? 0) - (a.count ?? 0)).slice(0, MAX_MASTER_ARROWS)
  const shapes: DrawShape[] = []
  for (const stat of top) {
    const mv = moves.find((m) => m.san === stat.san)
    if (!mv) continue
    const freq = Math.round((100 * (stat.count ?? 0)) / total)
    shapes.push({
      orig: mv.from as DrawShape['orig'],
      dest: mv.to as DrawShape['dest'],
      brush: 'master',
      modifiers: { lineWidth: widthForFrequency(freq) },
      label: { text: `${freq}%` },
    })
  }
  return shapes
}
