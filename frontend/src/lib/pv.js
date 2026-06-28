// Render engine principal variations (UCI long-algebraic) as readable SAN by
// replaying them from a starting FEN with chess.js. Pure + unit-tested.

import { Chess } from 'chess.js'

/** Split a UCI move (`e2e4`, `e7e8q`) into a chess.js move descriptor. */
export function parseUci(uci) {
  if (typeof uci !== 'string' || uci.length < 4) return null
  const move = { from: uci.slice(0, 2), to: uci.slice(2, 4) }
  if (uci.length > 4) move.promotion = uci[4].toLowerCase()
  return move
}

/**
 * Render a UCI principal variation as SAN moves, replaying from `fen`. Stops at
 * the first illegal move — engines can briefly emit a PV for the previous
 * position mid-search — and caps the output at `maxPlies`.
 */
export function uciLineToSan(fen, uciMoves, maxPlies = 12) {
  const out = []
  if (!Array.isArray(uciMoves)) return out
  let chess
  try {
    chess = new Chess(fen)
  } catch {
    return out
  }
  for (const uci of uciMoves.slice(0, maxPlies)) {
    const move = parseUci(uci)
    if (!move) break
    let res
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
