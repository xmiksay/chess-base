import { describe, it, expect } from 'vitest'
import { createMemoryHistory } from 'vue-router'
import { createAppRouter, routes } from './index.js'

describe('router', () => {
  it('defines a named route for every top-level surface', () => {
    const names = routes.map((r) => r.name)
    expect(names).toEqual(['analysis', 'collections', 'games', 'search', 'settings', 'login'])
  })

  it('maps / to the analysis view', () => {
    const analysis = routes.find((r) => r.path === '/')
    expect(analysis.name).toBe('analysis')
  })

  it('resolves each path to its route record', async () => {
    const router = createAppRouter(createMemoryHistory())
    for (const { path, name } of routes) {
      const resolved = router.resolve(path)
      expect(resolved.name).toBe(name)
    }
  })

  it('falls back through to the SPA for an unknown path', () => {
    const router = createAppRouter()
    // No catch-all is defined client-side; unknown paths resolve to an empty
    // match, which the server's index.html fallback hands to the client router.
    const resolved = router.resolve('/does-not-exist')
    expect(resolved.matched).toHaveLength(0)
  })
})
