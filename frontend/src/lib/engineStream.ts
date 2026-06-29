// Pure helpers for the engine-analysis WebSocket stream.
//
// The backend streams (see `src/server/engine_ws.rs`):
//   {"type":"ready","name":…}                     — engine spawned
//   {"type":"info", depth, multipv, score, nps, pv, …}
//   {"type":"bestmove","best_move":…,"ponder":…}  — search terminated
//   {"type":"error","message":…}
// `score` is `{type:"cp"|"mate", value}` from the side-to-move's perspective.
//
// Everything here is framework-free so the parsing/reduction is unit-testable.

import type { Color, EngineLine, EngineMessage, Score } from '../types'

type EngineInfo = Extract<EngineMessage, { type: 'info' }>

/**
 * Parse one raw WebSocket text frame into an engine event object.
 * Returns null for frames that aren't a JSON object carrying a string `type`.
 */
export function parseEngineMessage(data: string): EngineMessage | null {
  let msg
  try {
    msg = JSON.parse(data)
  } catch {
    return null
  }
  if (!msg || typeof msg !== 'object' || typeof msg.type !== 'string') return null
  return msg as EngineMessage
}

/**
 * Fold an `info` event into a Map of PV lines keyed by their 1-based MultiPV
 * index. Mutates and returns `lines`. Partial `info` lines that carry neither a
 * score nor a pv (e.g. `info depth 1 currmove …`) are ignored so they don't
 * blank an existing line.
 */
export function reduceInfo(
  lines: Map<number, EngineLine>,
  info: EngineInfo,
): Map<number, EngineLine> {
  if (!info || (info.score == null && (!info.pv || info.pv.length === 0))) {
    return lines
  }
  const idx = info.multipv ?? 1
  const prev: Partial<EngineLine> = lines.get(idx) || {}
  lines.set(idx, {
    multipv: idx,
    depth: info.depth ?? prev.depth ?? null,
    seldepth: info.seldepth ?? prev.seldepth ?? null,
    score: info.score ?? prev.score ?? null,
    nodes: info.nodes ?? prev.nodes ?? null,
    nps: info.nps ?? prev.nps ?? null,
    timeMs: info.time_ms ?? prev.timeMs ?? null,
    pv: info.pv && info.pv.length ? info.pv : (prev.pv ?? []),
  })
  return lines
}

/** PV lines as a plain array sorted by ascending MultiPV index. */
export function sortedLines(lines: Map<number, EngineLine>): EngineLine[] {
  return [...lines.values()].sort((a, b) => a.multipv - b.multipv)
}

/**
 * Convert an engine score (side-to-move relative) to centipawns from White's
 * perspective. Mates map to a large finite magnitude (shorter mate ⇒ larger)
 * so the value sorts and clamps sanely.
 */
export function scoreToWhiteCp(
  score: Score | null | undefined,
  sideToMove: Color,
): number {
  if (!score) return 0
  const sign = sideToMove === 'black' ? -1 : 1
  if (score.type === 'mate') {
    const mag = 100000 - Math.min(Math.abs(score.value), 1000) * 10
    return sign * (score.value >= 0 ? mag : -mag)
  }
  return sign * score.value
}

/** Human-readable score from White's perspective: "+1.24", "-0.37", "M5", "-M3". */
export function formatScore(
  score: Score | null | undefined,
  sideToMove: Color,
): string {
  if (!score) return '—'
  const sign = sideToMove === 'black' ? -1 : 1
  if (score.type === 'mate') {
    const v = sign * score.value
    return v >= 0 ? `M${Math.abs(v)}` : `-M${Math.abs(v)}`
  }
  const cp = (sign * score.value) / 100
  return (cp > 0 ? '+' : '') + cp.toFixed(2)
}

/**
 * White win-probability as a 0–100 percentage, used for the eval-bar height.
 * Uses the standard logistic mapping cp → win% (≈400 cp per decade).
 */
export function evalBarPercent(
  score: Score | null | undefined,
  sideToMove: Color,
): number {
  if (!score) return 50
  if (score.type === 'mate') {
    return scoreToWhiteCp(score, sideToMove) >= 0 ? 100 : 0
  }
  const cp = scoreToWhiteCp(score, sideToMove)
  const wp = 1 / (1 + Math.pow(10, -cp / 400))
  return Math.max(0, Math.min(100, wp * 100))
}
