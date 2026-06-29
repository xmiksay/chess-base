// Pure header-search query state — framework-free and unit-tested. The form binds
// to a flat query object; this module owns the empty shape, the "is anything
// set?" check, and the mapping to the backend's snake_case query params (blank
// fields dropped so they don't constrain the search). Mirrors the `/api/search/
// headers` filter surface (issue #6): a player name + optional color, event, ECO
// prefix, result and an inclusive PGN date range.

import type { HeaderQuery } from '../types'

/** Fields the header-search form drives. Order matches the form layout. */
export const QUERY_FIELDS: (keyof HeaderQuery)[] = [
  'player',
  'color',
  'event',
  'result',
  'eco',
  'dateFrom',
  'dateTo',
]

/** Camel-case form field → backend query-param name. */
const PARAM_OF: Record<keyof HeaderQuery, string> = {
  player: 'player',
  color: 'color',
  event: 'event',
  result: 'result',
  eco: 'eco',
  dateFrom: 'date_from',
  dateTo: 'date_to',
}

/** A fresh, all-blank query object. */
export function emptyQuery(): HeaderQuery {
  return QUERY_FIELDS.reduce((q, f) => ({ ...q, [f]: '' }), {} as HeaderQuery)
}

/** True when no field carries a (non-whitespace) value. */
export function isEmptyQuery(query: HeaderQuery): boolean {
  return QUERY_FIELDS.every((f) => !String(query[f] ?? '').trim())
}

/**
 * Backend query params for a search: every non-blank field, trimmed, keyed by its
 * snake_case param name. An empty query yields `{}` (lists all visible games).
 */
export function toParams(query: HeaderQuery): Record<string, string> {
  const params: Record<string, string> = {}
  for (const f of QUERY_FIELDS) {
    const value = String(query[f] ?? '').trim()
    if (value) params[PARAM_OF[f]] = value
  }
  return params
}

/** A URL query string (no leading `?`) for the given query, blanks dropped. */
export function toQueryString(query: HeaderQuery): string {
  return new URLSearchParams(toParams(query)).toString()
}
