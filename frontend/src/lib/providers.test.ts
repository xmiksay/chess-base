import { describe, it, expect } from 'vitest'
import { effectiveDefault, modelOptions } from './providers'
import type { ProviderInfo } from '../types'

function row(over: Partial<ProviderInfo> = {}): ProviderInfo {
  return {
    id: 1,
    name: 'anthropic',
    wire: 'anthropic',
    model: 'claude-a',
    base_url: null,
    has_key: true,
    is_default: false,
    is_global: false,
    models: ['claude-a'],
    ...over,
  }
}

describe('effectiveDefault', () => {
  it('prefers the own default over a global default', () => {
    const rows = [
      row({ id: 9, name: 'global', is_global: true, is_default: true }),
      row({ id: 3, name: 'own', is_default: true }),
    ]
    expect(effectiveDefault(rows)?.name).toBe('own')
  })

  it('falls back to the global default when no own row is flagged', () => {
    const rows = [
      row({ id: 9, name: 'global', is_global: true, is_default: true }),
      row({ id: 3, name: 'own' }),
    ]
    expect(effectiveDefault(rows)?.name).toBe('global')
  })

  it('with no flags picks the own lowest-id row despite id-descending input', () => {
    // The API lists newest (highest id) first — positional "first" would be wrong.
    const rows = [
      row({ id: 7, name: 'newest-own' }),
      row({ id: 2, name: 'oldest-own' }),
      row({ id: 1, name: 'global', is_global: true }),
    ]
    expect(effectiveDefault(rows)?.name).toBe('oldest-own')
  })

  it('with only globals picks the global lowest-id row', () => {
    const rows = [
      row({ id: 8, name: 'g-new', is_global: true }),
      row({ id: 4, name: 'g-old', is_global: true }),
    ]
    expect(effectiveDefault(rows)?.name).toBe('g-old')
  })

  it('returns null for an empty listing', () => {
    expect(effectiveDefault([])).toBeNull()
  })
})

describe('modelOptions', () => {
  it('groups own rows first (id-ascending), then unshadowed globals', () => {
    const rows = [
      row({ id: 9, name: 'zai', models: ['glm-5'] }),
      row({ id: 5, name: 'anthropic', models: ['claude-a', 'claude-b'] }),
      // Shadowed by the own "anthropic" row above — the resolver ignores it.
      row({ id: 2, name: 'anthropic', is_global: true, models: ['claude-g'] }),
      row({ id: 1, name: 'house', is_global: true, models: ['house-m'] }),
    ]
    expect(modelOptions(rows)).toEqual([
      { provider: 'anthropic', models: ['claude-a', 'claude-b'] },
      { provider: 'zai', models: ['glm-5'] },
      { provider: 'house', models: ['house-m'] },
    ])
  })

  it('falls back to the single own model when models is missing or empty', () => {
    const bare = { ...row({ model: 'solo' }), models: [] }
    expect(modelOptions([bare])).toEqual([{ provider: 'anthropic', models: ['solo'] }])
  })
})
