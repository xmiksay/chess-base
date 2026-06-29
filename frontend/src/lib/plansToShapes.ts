// Pure: turn engine PlanLines (per-piece trajectories) into chessground
// auto-shapes — one color per line, chained arrows along each piece's path.
//
// Each line gets its own brush (`plan1…plan3`) plus a dimmed variant (`…d`)
// used for every non-active line. Framework-free apart from chessground's
// shape/brush *types*, so the mapping is unit-testable in isolation.
// (Issue #60; trajectories come from `src/plans.rs`, ADR-0017.)

import type { DrawShape, DrawBrush } from 'chessground/draw'
import type { PlanLine } from '../types'

/** Max overlaid plan lines (top MultiPV); more would clutter the board. */
export const MAX_PLAN_LINES = 3

/** Per-line base color, indexed by rank (best line first). */
const PLAN_COLORS = ['#15803d', '#2563eb', '#c2410c']

const FULL_OPACITY = 0.9
const DIM_OPACITY = 0.25

/**
 * chessground brush table for the plan overlay: `plan1…plan3` at full opacity
 * plus dimmed `plan1d…plan3d`. Spread into a board's `drawable.brushes`.
 */
export function planBrushes(): Record<string, DrawBrush> {
  const brushes: Record<string, DrawBrush> = {}
  PLAN_COLORS.forEach((color, i) => {
    const n = i + 1
    brushes[`plan${n}`] = { key: `pl${n}`, color, opacity: FULL_OPACITY, lineWidth: 10 }
    brushes[`plan${n}d`] = { key: `pl${n}d`, color, opacity: DIM_OPACITY, lineWidth: 8 }
  })
  return brushes
}

export interface PlansToShapesOptions {
  /** MultiPV of the active (hovered) line; every other line is dimmed. */
  active?: number | null
  /** Add a 1-based ply-order label on each piece's first arrow. */
  labels?: boolean
}

/**
 * Build chessground auto-shapes for the top plan lines. Every line draws one
 * arrow per consecutive square pair of each piece trajectory (`g1→f3→g5` ⇒
 * `g1→f3`, `f3→g5`), brushed by the line's rank. With `active` set, that line
 * keeps full opacity and the others switch to their dimmed brush. Lines are
 * ranked by ascending MultiPV and capped at [`MAX_PLAN_LINES`].
 */
export function plansToShapes(
  lines: PlanLine[],
  { active = null, labels = false }: PlansToShapesOptions = {},
): DrawShape[] {
  if (!Array.isArray(lines)) return []
  const top = [...lines]
    .filter((l) => l && Array.isArray(l.trajectories))
    .sort((a, b) => a.multipv - b.multipv)
    .slice(0, MAX_PLAN_LINES)

  const shapes: DrawShape[] = []
  top.forEach((line, rank) => {
    const base = `plan${rank + 1}`
    const brush = active != null && line.multipv !== active ? `${base}d` : base
    line.trajectories.forEach((traj, order) => {
      const sq = traj?.squares
      if (!Array.isArray(sq)) return
      for (let i = 0; i + 1 < sq.length; i++) {
        const orig = sq[i]
        const dest = sq[i + 1]
        if (!orig || !dest || orig === dest) continue
        const shape = { orig, dest, brush } as DrawShape
        // Only the first segment carries the label so the move order reads once.
        if (labels && i === 0) shape.label = { text: String(order + 1) }
        shapes.push(shape)
      }
    })
  })
  return shapes
}
