// Pure opening-tree navigation + move-stat math — framework-free and unit-tested.
// The explorer models the current position as a *line*: an ordered list of SAN
// moves from the start position. Replaying that line with chess.js yields the
// FEN to query and the legal moves to offer on the board; clicking a tree row
// appends one SAN, "back" pops one. All of this is pure, so it tests without a
// DOM or a store.

import { Chess, type Move } from 'chess.js'
import { STARTPOS_FEN } from './fen'
import type { Dests, MoveStat, ReplayPosition, Square } from '../types'

export const START_FEN = STARTPOS_FEN

/** Legal-move map for chessground from a chess.js instance: from → [to, …]. */
export function legalDests(chess: Chess): Dests {
  const map: Dests = new Map()
  for (const m of chess.moves({ verbose: true })) {
    const arr = map.get(m.from)
    if (arr) arr.push(m.to)
    else map.set(m.from, [m.to])
  }
  return map
}

/**
 * Replay a line of SAN moves from the start position. Stops at the first illegal
 * move (so a bad line never throws). Returns the reached position's `fen`, the
 * `dests` map, the `lastMove` as `[from, to]` (or null at the root), the side to
 * move, and `ok` (false if a move was rejected — `plies` then counts those that
 * actually applied).
 */
export function replayLine(sans: string[]): ReplayPosition {
  const chess = new Chess()
  let lastMove: [Square, Square] | null = null
  let ok = true
  let plies = 0
  for (const san of sans) {
    let move: Move | null
    try {
      move = chess.move(san)
    } catch {
      move = null
    }
    if (!move) {
      ok = false
      break
    }
    lastMove = [move.from, move.to]
    plies += 1
  }
  return {
    fen: chess.fen(),
    dests: legalDests(chess),
    lastMove,
    turnColor: chess.turn() === 'b' ? 'black' : 'white',
    plies,
    ok,
  }
}

/** The FEN reached by replaying `sans` (illegal tail ignored). */
export function lineFen(sans: string[]): string {
  return replayLine(sans).fen
}

/**
 * Translate a board drag `{from,to}` at the position reached by `sans` into its
 * SAN, or null when the move is illegal. `promotion` defaults to a queen.
 */
export function moveToSan(
  sans: string[],
  from: Square,
  to: Square,
  promotion: string = 'q',
): string | null {
  const { chess } = replayInstance(sans)
  let move: Move | null
  try {
    move = chess.move({ from, to, promotion })
  } catch {
    return null
  }
  return move ? move.san : null
}

/** Internal: a chess.js instance advanced to the (legal prefix of the) line. */
function replayInstance(sans: string[]): { chess: Chess } {
  const chess = new Chess()
  for (const san of sans) {
    try {
      if (!chess.move(san)) break
    } catch {
      break
    }
  }
  return { chess }
}

/** Total games across an opening-tree row set. */
export function totalCount(stats: MoveStat[]): number {
  return stats.reduce((sum, s) => sum + (s.count ?? 0), 0)
}

/**
 * Result split for one move as integer percentages summing to ~100 over the
 * *decided* games (white wins / draws / black wins). Games with an unknown
 * result (`*`) are excluded from the denominator. An all-unknown row reads 0/0/0.
 */
export function scoreBar(stat: MoveStat): { white: number; draws: number; black: number } {
  const decided = (stat.white ?? 0) + (stat.draws ?? 0) + (stat.black ?? 0)
  if (decided === 0) return { white: 0, draws: 0, black: 0 }
  const white = Math.round((100 * (stat.white ?? 0)) / decided)
  const black = Math.round((100 * (stat.black ?? 0)) / decided)
  return { white, draws: 100 - white - black, black }
}

/** This move's share of all games at the position, as an integer percentage. */
export function frequency(stat: MoveStat, total: number): number {
  if (!total) return 0
  return Math.round((100 * (stat.count ?? 0)) / total)
}

/**
 * A compact "N games, WW/DD/LL" stat string for a move, e.g. for the "Add line
 * to study" comment (issue #173).
 */
export function formatMoveStat(stat: MoveStat): string {
  const games = stat.count ?? 0
  const plural = games === 1 ? 'game' : 'games'
  return `${games} ${plural}, ${stat.white ?? 0}W/${stat.draws ?? 0}D/${stat.black ?? 0}L`
}
