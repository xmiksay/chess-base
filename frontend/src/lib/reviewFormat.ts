// Pure formatting helpers for the engine-only game review (issue #119, Mode A).
// The review's eval/mate are already White-perspective numbers, so format them
// directly rather than going through the side-to-move `Score` helpers.

import type { MoveClassification, MoveReview } from '../types'

/** White-perspective eval as a signed string: "+1.20", "-2.60", "M3", "-M3". */
export function formatReviewEval(move: Pick<MoveReview, 'eval_cp' | 'mate'>): string {
  if (move.mate != null) {
    return move.mate >= 0 ? `M${move.mate}` : `-M${Math.abs(move.mate)}`
  }
  const cp = move.eval_cp / 100
  return (cp > 0 ? '+' : '') + cp.toFixed(2)
}

/** The annotation glyph for a move quality, or '' for clean moves. */
export function classificationGlyph(c: MoveClassification): string {
  switch (c) {
    case 'blunder':
      return '??'
    case 'mistake':
      return '?'
    case 'inaccuracy':
      return '?!'
    case 'great':
      return '!'
    default:
      return ''
  }
}

/** Tailwind text-colour class for a move quality (subtle accent for good play). */
export function classificationClass(c: MoveClassification): string {
  switch (c) {
    case 'blunder':
      return 'text-red-600'
    case 'mistake':
      return 'text-orange-500'
    case 'inaccuracy':
      return 'text-yellow-600'
    case 'great':
      return 'text-emerald-600'
    case 'best':
      return 'text-emerald-500'
    default:
      return ''
  }
}
