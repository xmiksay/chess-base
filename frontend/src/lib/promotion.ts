// Detect a promoting move so the board can ask which piece.
//
// `useTreeBoard.playMove` has always accepted a `promotion`, but every caller
// defaulted it to `'q'` and the board only ever emitted `{from, to}` — so
// underpromotion was unreachable in the UI. This is the missing predicate.

import { sideToMove } from './fen'
import type { Square } from '../types'

/** The pieces a pawn may become. `q` first: it is the overwhelming default. */
export const PROMOTION_PIECES = ['q', 'r', 'b', 'n'] as const
export type PromotionPiece = (typeof PROMOTION_PIECES)[number]

/**
 * Whether moving `from` → `to` in `fen` promotes: a pawn of the side to move
 * reaching its last rank. Purely positional — legality is the board's job, so a
 * non-pawn or an empty origin square simply answers false.
 */
export function isPromotion(fen: string, from: Square, to: Square): boolean {
  const mover = sideToMove(fen)
  const lastRank = mover === 'white' ? '8' : '1'
  if (to[1] !== lastRank) return false
  return pieceAt(fen, from) === (mover === 'white' ? 'P' : 'p')
}

/** The piece letter on `square` in FEN notation, or null when it is empty. */
function pieceAt(fen: string, square: Square): string | null {
  const board = fen.split(' ')[0]
  const file = square.charCodeAt(0) - 'a'.charCodeAt(0)
  const rank = Number(square[1])
  if (file < 0 || file > 7 || !Number.isInteger(rank) || rank < 1 || rank > 8) return null

  // FEN ranks run 8 → 1, so rank 8 is the first row.
  const row = board.split('/')[8 - rank]
  if (!row) return null

  let f = 0
  for (const ch of row) {
    const skip = Number(ch)
    if (Number.isInteger(skip) && skip > 0) {
      f += skip
      if (f > file) return null // the square falls inside an empty run
      continue
    }
    if (f === file) return ch
    f++
  }
  return null
}
