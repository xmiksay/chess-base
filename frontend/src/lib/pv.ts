// Render engine principal variations (UCI long-algebraic) as readable SAN by
// replaying them from a starting FEN with chess.js. Pure + unit-tested.

import { Chess, type Move } from 'chess.js'
import type { Square } from '../types'

/** Split a UCI move (`e2e4`, `e7e8q`) into a chess.js move descriptor. */
export function parseUci(uci: string): { from: Square; to: Square; promotion?: string } | null {
  if (typeof uci !== 'string' || uci.length < 4) return null
  const move: { from: Square; to: Square; promotion?: string } = {
    from: uci.slice(0, 2),
    to: uci.slice(2, 4),
  }
  if (uci.length > 4) move.promotion = uci[4].toLowerCase()
  return move
}

/**
 * Render a UCI principal variation as SAN moves, replaying from `fen`. Stops at
 * the first illegal move — engines can briefly emit a PV for the previous
 * position mid-search — and caps the output at `maxPlies`.
 */
export function uciLineToSan(fen: string, uciMoves: string[], maxPlies: number = 12): string[] {
  const out: string[] = []
  if (!Array.isArray(uciMoves)) return out
  let chess: Chess
  try {
    chess = new Chess(fen)
  } catch {
    return out
  }
  for (const uci of uciMoves.slice(0, maxPlies)) {
    const move = parseUci(uci)
    if (!move) break
    let res: Move | null
    try {
      res = chess.move(move)
    } catch {
      break
    }
    if (!res) break
    out.push(res.san)
  }
  return out
}
