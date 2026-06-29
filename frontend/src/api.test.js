import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'
import { api, setAuthToken, getAuthToken } from './api.js'

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
