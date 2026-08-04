import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'
import { api, setAuthToken, getAuthToken } from './api'

const TOKEN_KEY = 'chess-base:token'

function mockFetch({ status = 200, body = {} } = {}) {
  const fn = vi.fn().mockResolvedValue({
    ok: status >= 200 && status < 300,
    status,
    json: async () => body,
    text: async () => '',
  })
  vi.stubGlobal('fetch', fn)
  return fn
}

beforeEach(() => {
  window.localStorage.clear()
  setAuthToken(null)
})

afterEach(() => {
  vi.unstubAllGlobals()
})

describe('api token attachment', () => {
  it('omits the Authorization header when no token is set', async () => {
    const fetchMock = mockFetch()
    await api.whoami()
    const [, opts] = fetchMock.mock.calls[0]
    expect(opts.headers.Authorization).toBeUndefined()
  })

  it('attaches a Bearer header on GET requests once a token is set', async () => {
    setAuthToken('tok42')
    const fetchMock = mockFetch()
    await api.whoami()
    const [, opts] = fetchMock.mock.calls[0]
    expect(opts.headers.Authorization).toBe('Bearer tok42')
  })

  it('attaches the Bearer header alongside Content-Type on POST requests', async () => {
    setAuthToken('tok42')
    const fetchMock = mockFetch({ status: 200, body: { ok: true } })
    await api.auth.login('alice', 'password1')
    const [path, opts] = fetchMock.mock.calls[0]
    expect(path).toBe('/api/auth/login')
    expect(opts.method).toBe('POST')
    expect(opts.headers.Authorization).toBe('Bearer tok42')
    expect(opts.headers['Content-Type']).toBe('application/json')
  })

  it('persists the token to localStorage and exposes it via getAuthToken', () => {
    setAuthToken('persisted')
    expect(getAuthToken()).toBe('persisted')
    expect(window.localStorage.getItem(TOKEN_KEY)).toBe('persisted')
  })

  it('clears the token from memory and storage when set to null', () => {
    setAuthToken('persisted')
    setAuthToken(null)
    expect(getAuthToken()).toBe(null)
    expect(window.localStorage.getItem(TOKEN_KEY)).toBe(null)
  })
})

// The analyse body must carry exactly the fields the caller set (#216): sending
// `plan_lines`/`threats` — even 0/false — opts into the #191 shape
// regeneration, so an absent field has different semantics than a default one.
describe('api sharing toggles (#213)', () => {
  it('PUTs { public } to the game toggle route', async () => {
    const fetchMock = mockFetch({ status: 200, body: { id: 1, public: true } })
    await api.games.setPublic(1, true)
    const [path, opts] = fetchMock.mock.calls[0]
    expect(path).toBe('/api/games/1/public')
    expect(opts.method).toBe('PUT')
    expect(JSON.parse(opts.body)).toEqual({ public: true })
  })

  it('PUTs { public } to the study toggle route', async () => {
    const fetchMock = mockFetch({ status: 200, body: { id: 1, public: false } })
    await api.studies.setPublic(1, false)
    const [path, opts] = fetchMock.mock.calls[0]
    expect(path).toBe('/api/studies/1/public')
    expect(opts.method).toBe('PUT')
    expect(JSON.parse(opts.body)).toEqual({ public: false })
  })
})

describe('api.studies.analyse body', () => {
  it('sends an empty body by default and only the fields the caller set', async () => {
    const fetchMock = mockFetch()
    await api.studies.analyse(5)
    const [path, opts] = fetchMock.mock.calls[0]
    expect(path).toBe('/api/studies/5/analyse')
    expect(JSON.parse(opts.body)).toEqual({})

    await api.studies.analyse(5, { depth: 20 })
    const [, second] = fetchMock.mock.calls[1]
    expect(JSON.parse(second.body)).toEqual({ depth: 20 })
  })

  it('keeps explicit 0/false shape fields — the strip-stale-arrows opt-in', async () => {
    const fetchMock = mockFetch()
    await api.studies.analyse(5, { plan_lines: 0, threats: false })
    const [, opts] = fetchMock.mock.calls[0]
    expect(JSON.parse(opts.body)).toEqual({ plan_lines: 0, threats: false })
  })
})
