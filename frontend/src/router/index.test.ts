import { describe, it, expect } from 'vitest'
import { createMemoryHistory } from 'vue-router'
import { createAppRouter, routes, authRedirect } from './index'

describe('router', () => {
  it('defines a named route for every top-level surface', () => {
    const names = routes.map((r) => r.name)
    expect(names).toEqual([
      'analysis',
      'collections',
      'games',
      'studies',
      'assistant',
      'import',
      'search',
      'settings',
      'login',
    ])
  })

  it('maps / to the analysis view', () => {
    const analysis = routes.find((r) => r.path === '/')
    expect(analysis!.name).toBe('analysis')
  })

  it('resolves each bare path to its route record', async () => {
    const router = createAppRouter(createMemoryHistory())
    for (const { path, name } of routes) {
      // Strip the optional object-id param (issue #212): the bare surface path
      // must keep resolving to the same named route.
      const bare = path.replace(/\/:.*$/, '') || '/'
      const resolved = router.resolve(bare)
      expect(resolved.name).toBe(name)
    }
  })

  // Issue #212: every selectable object is URL-addressable via an optional param.
  it('resolves object ids into the games, studies and assistant routes', () => {
    const router = createAppRouter(createMemoryHistory())

    const game = router.resolve('/games/123')
    expect(game.name).toBe('games')
    expect(game.params).toEqual({ id: '123' })

    const study = router.resolve('/studies/45')
    expect(study.name).toBe('studies')
    expect(study.params).toEqual({ id: '45' })

    const session = router.resolve('/assistant/abc')
    expect(session.name).toBe('assistant')
    expect(session.params).toEqual({ sessionId: 'abc' })
  })

  it('keeps the db query on a games deep link', () => {
    const router = createAppRouter(createMemoryHistory())
    const resolved = router.resolve('/games/5?db=2')
    expect(resolved.name).toBe('games')
    expect(resolved.params).toEqual({ id: '5' })
    expect(resolved.query).toEqual({ db: '2' })
  })

  it('falls back through to the SPA for an unknown path', () => {
    const router = createAppRouter()
    // No catch-all is defined client-side; unknown paths resolve to an empty
    // match, which the server's index.html fallback hands to the client router.
    const resolved = router.resolve('/does-not-exist')
    expect(resolved.matched).toHaveLength(0)
  })
})

describe('authRedirect', () => {
  it('bounces a gated navigation to /login with the original path', () => {
    const r = authRedirect(
      { name: 'search', fullPath: '/search', params: {} },
      { needsAuth: true, isServerMode: true },
    )
    expect(r).toEqual({ name: 'login', query: { redirect: '/search' } })
  })

  it('lets a server-mode caller reach /login without a session', () => {
    const r = authRedirect(
      { name: 'login', fullPath: '/login', params: {} },
      { needsAuth: true, isServerMode: true },
    )
    expect(r).toBe(null)
  })

  it('sends an already-signed-in user away from /login', () => {
    const r = authRedirect(
      { name: 'login', fullPath: '/login', params: {} },
      { needsAuth: false, isServerMode: true },
    )
    expect(r).toEqual({ name: 'analysis' })
  })

  it('never gates in local mode', () => {
    expect(
      authRedirect(
        { name: 'settings', fullPath: '/settings', params: {} },
        { needsAuth: false, isServerMode: false },
      ),
    ).toBe(null)
    expect(
      authRedirect(
        { name: 'login', fullPath: '/login', params: {} },
        { needsAuth: false, isServerMode: false },
      ),
    ).toBe(null)
  })

  // Issue #213: a public game/study deep link must stay reachable logged-out.
  it('lets an anonymous deep link with an id through on games/studies', () => {
    expect(
      authRedirect(
        { name: 'games', fullPath: '/games/5', params: { id: '5' } },
        { needsAuth: true, isServerMode: true },
      ),
    ).toBe(null)
    expect(
      authRedirect(
        { name: 'studies', fullPath: '/studies/9', params: { id: '9' } },
        { needsAuth: true, isServerMode: true },
      ),
    ).toBe(null)
  })

  it('still gates the bare games/studies list (no id) when logged out', () => {
    expect(
      authRedirect(
        { name: 'games', fullPath: '/games', params: {} },
        { needsAuth: true, isServerMode: true },
      ),
    ).toEqual({ name: 'login', query: { redirect: '/games' } })
    expect(
      authRedirect(
        { name: 'studies', fullPath: '/studies', params: {} },
        { needsAuth: true, isServerMode: true },
      ),
    ).toEqual({ name: 'login', query: { redirect: '/studies' } })
  })

  it('still gates an id deep link on a route outside the anonymous allowlist', () => {
    expect(
      authRedirect(
        { name: 'assistant', fullPath: '/assistant/abc', params: { sessionId: 'abc' } },
        { needsAuth: true, isServerMode: true },
      ),
    ).toEqual({ name: 'login', query: { redirect: '/assistant/abc' } })
  })
})
