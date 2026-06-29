// Replay a stored game's PGN into a list of board positions, and the pure
// ply-navigation logic the viewer steps through. Framework-free + unit-tested.

import { Chess } from 'chess.js'
import { STARTPOS_FEN } from './fen'
import type { ViewerPosition } from '../types'

/**
 * Replay a PGN into one entry per position, index 0 being the start position
 * (before any move). Each later entry carries the move that produced it:
 *   { ply, san, fen, lastMove: [from, to] }
 * `ply` 0 is the start; `lastMove` is null there. Returns a single start-only
 * entry if the PGN is empty or unparseable, so the caller always has a board.
 */
export function positionsFromPgn(pgn: string | null | undefined): ViewerPosition[] {
  const chess = new Chess()
  let loaded = false
  try {
    chess.loadPgn(String(pgn ?? ''))
    loaded = true
  } catch {
    loaded = false
  }

  const verbose = loaded ? chess.history({ verbose: true }) : []
  // `before` of the first move is the true start FEN (handles SetUp/FEN PGNs);
  // fall back to the standard startpos for an empty game.
  const startFen = verbose.length ? verbose[0].before : loaded ? chess.fen() : STARTPOS_FEN

  const positions: ViewerPosition[] = [{ ply: 0, san: null, fen: startFen, lastMove: null }]
  verbose.forEach((m, i) => {
    positions.push({
      ply: i + 1,
      san: m.san,
      fen: m.after,
      lastMove: [m.from, m.to],
    })
  })
  return positions
}

/** Clamp a ply index into `[0, total - 1]` for a position list of `total` entries. */
export function clampPly(ply: number, total: number): number {
  if (total <= 0) return 0
  if (!Number.isFinite(ply)) return 0
  return Math.min(Math.max(Math.trunc(ply), 0), total - 1)
}

/**
 * Resolve a navigation step against the current ply.
 * `action` ∈ 'first' | 'prev' | 'next' | 'last', or a number to go to that ply.
 * Always returns a valid in-range ply (clamped).
 */
export function navigate(current: number, action: string | number, total: number): number {
  let target
  switch (action) {
    case 'first':
      target = 0
      break
    case 'last':
      target = total - 1
      break
    case 'prev':
      target = current - 1
      break
    case 'next':
      target = current + 1
      break
    default:
      target = Number(action)
  }
  return clampPly(target, total)
}
