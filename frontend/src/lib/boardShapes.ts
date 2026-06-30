// Pure board-overlay composition (issue #123). The board renders the *union* of
// the enabled overlay layers — Plans (engine-PV trajectories, #60), Threats
// (hanging pieces) and Database master moves — composed in one place instead of
// each source writing to the board directly. Framework-free apart from
// chessground's shape/brush *types*, so it unit-tests in isolation.

import type { DrawShape, DrawBrush } from 'chessground/draw'
import type { Shape } from '../types'

/** The three independently-toggleable overlay layers. */
export interface BoardLayers {
  plans?: DrawShape[]
  threats?: DrawShape[]
  master?: DrawShape[]
}

/** Which layers are currently enabled (mirrors the persisted user settings). */
export interface LayerToggles {
  plans: boolean
  threats: boolean
  master: boolean
}

/**
 * Union of the enabled layers' shapes, in a stable order (plans, then threats,
 * then master) so a disabled layer contributes nothing to the board.
 */
export function composeBoardShapes(layers: BoardLayers, on: LayerToggles): DrawShape[] {
  const out: DrawShape[] = []
  if (on.plans) out.push(...(layers.plans ?? []))
  if (on.threats) out.push(...(layers.threats ?? []))
  if (on.master) out.push(...(layers.master ?? []))
  return out
}

/** Map stored `Shape`s (the backend threats payload) onto chessground shapes. */
export function shapesToDrawShapes(shapes: Shape[]): DrawShape[] {
  if (!Array.isArray(shapes)) return []
  return shapes.map((s) => {
    const shape = { orig: s.orig, brush: s.brush } as DrawShape
    if (s.dest) shape.dest = s.dest as DrawShape['dest']
    return shape
  })
}

const THREAT_COLOR = '#dc2626' // red-600
const MASTER_COLOR = '#7c3aed' // violet-600

/**
 * Brush table for the non-plan overlays: a red `threat` brush and a violet
 * `master` brush. Spread alongside `planBrushes()` into a board's
 * `drawable.brushes`. (Master-arrow *thickness* is varied per-shape via the
 * shape's `modifiers.lineWidth`, so one brush key suffices.)
 */
export function overlayBrushes(): Record<string, DrawBrush> {
  return {
    threat: { key: 'thr', color: THREAT_COLOR, opacity: 0.9, lineWidth: 10 },
    master: { key: 'mst', color: MASTER_COLOR, opacity: 0.85, lineWidth: 10 },
  }
}
