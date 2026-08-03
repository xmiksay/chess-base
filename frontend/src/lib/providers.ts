// Pure helpers over the LLM provider listing (issue #214), shared by the
// study-generation dialogs' ModelSelect (and later the assistant's picker).
//
// The API lists rows id-DESCENDING (newest first), so "first row" fallbacks
// must select by lowest id, never by position.

import type { ProviderInfo } from '../types'

/** An explicit provider/model pick; `null` everywhere means "use the default". */
export interface ModelChoice {
  provider: string
  model: string
}

/** One picker group: a provider row's name and its selectable models. */
export interface ModelOptionGroup {
  provider: string
  models: string[]
}

/**
 * The provider row a run uses when the caller picks nothing — mirrors the
 * backend resolver's precedence (`owner_default` in the agent provider store):
 * own default → global default → own lowest-id → global lowest-id.
 */
export function effectiveDefault(rows: ProviderInfo[]): ProviderInfo | null {
  const own = rows.filter((r) => !r.is_global)
  const global = rows.filter((r) => r.is_global)
  const lowestId = (xs: ProviderInfo[]): ProviderInfo | null =>
    xs.length ? xs.reduce((a, b) => (b.id < a.id ? b : a)) : null
  return (
    own.find((r) => r.is_default) ??
    global.find((r) => r.is_default) ??
    lowestId(own) ??
    lowestId(global)
  )
}

/**
 * Flatten rows into grouped (provider → models) picker options. Own rows come
 * first (id-ascending), then global rows whose name an own row hasn't shadowed
 * — the same name-collision rule the backend resolver applies. A row without
 * an enriched `models` list falls back to its single own model.
 */
export function modelOptions(rows: ProviderInfo[]): ModelOptionGroup[] {
  const byId = (a: ProviderInfo, b: ProviderInfo) => a.id - b.id
  const own = rows.filter((r) => !r.is_global).sort(byId)
  const ownNames = new Set(own.map((r) => r.name))
  const global = rows
    .filter((r) => r.is_global && !ownNames.has(r.name))
    .sort(byId)
  return [...own, ...global].map((r) => ({
    provider: r.name,
    models: r.models?.length ? r.models : [r.model],
  }))
}
