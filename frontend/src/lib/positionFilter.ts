// Pure position-explorer filter state — framework-free and unit-tested. Mirrors
// lib/headerQuery's shape/empty/params trio, scoped to the smaller player/color/
// date-range filter applied to the opening-tree/games explorer and the study
// generator (issue #172): a player name + optional color + an inclusive PGN
// date range. Blank fields are dropped so they don't constrain the search.

import type { PositionFilter } from '../types'

/** Fields the position-filter form drives. Order matches the form layout. */
export const FILTER_FIELDS: (keyof PositionFilter)[] = ['player', 'color', 'dateFrom', 'dateTo']

/** Camel-case form field → backend query-param name. */
const PARAM_OF: Record<keyof PositionFilter, string> = {
  player: 'player',
  color: 'color',
  dateFrom: 'date_from',
  dateTo: 'date_to',
}

/** A fresh, all-blank filter object. */
export function emptyFilter(): PositionFilter {
  return FILTER_FIELDS.reduce((f, k) => ({ ...f, [k]: '' }), {} as PositionFilter)
}

/** True when no field carries a (non-whitespace) value. */
export function isEmptyFilter(filter: PositionFilter): boolean {
  return FILTER_FIELDS.every((f) => !String(filter[f] ?? '').trim())
}

/**
 * Backend query params for a filter: every non-blank field, trimmed, keyed by
 * its snake_case param name. An empty filter yields `{}` (no narrowing).
 */
export function toParams(filter: PositionFilter): Record<string, string> {
  const params: Record<string, string> = {}
  for (const f of FILTER_FIELDS) {
    const value = String(filter[f] ?? '').trim()
    if (value) params[PARAM_OF[f]] = value
  }
  return params
}
