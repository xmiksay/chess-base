import { describe, it, expect, vi, beforeEach } from 'vitest'
import { setActivePinia, createPinia } from 'pinia'
import { useAuthStore } from './auth'
import { api, setAuthToken, getAuthToken } from '../api'

vi.mock('../api', () => ({
  api: {
    health: vi.fn(),
    whoami: vi.fn(),
    auth: { register: vi.fn(), login: vi.fn(), logout: vi.fn() },
  },
  setAuthToken: vi.fn(),
  getAuthToken: vi.fn(),
}))

const USER_KEY = 'chess-base:user'

beforeEach(() => {
  vi.clearAllMocks()
  window.localStorage.clear()
  vi.mocked(getAuthToken).mockReturnValue(null)
  setActivePinia(createPinia())
})

describe('auth store — init()', () => {
  it('treats local mode as always authenticated, never gating', async () => {
    vi.mocked(api.health).mockResolvedValue({ mode: 'local' })
    const s = useAuthStore()
    await s.init()
    expect(s.isServerMode).toBe(false)
    expect(s.isAuthenticated).toBe(true)
    expect(s.needsAuth).toBe(false)
    expect(api.whoami).not.toHaveBeenCalled()
  })

  it('gates server mode when there is no token', async () => {
    vi.mocked(api.health).mockResolvedValue({ mode: 'server' })
    vi.mocked(getAuthToken).mockReturnValue(null)
    const s = useAuthStore()
    await s.init()
    expect(s.isServerMode).toBe(true)
    expect(s.needsAuth).toBe(true)
    expect(s.user).toBe(null)
  })

  it('restores the user from whoami when a token is present', async () => {
    vi.mocked(api.health).mockResolvedValue({ mode: 'server' })
    vi.mocked(getAuthToken).mockReturnValue('tok')
    vi.mocked(api.whoami).mockResolvedValue({ id: 'u1', is_admin: true })
    const s = useAuthStore()
    await s.init()
    expect(s.user).toEqual({ id: 'u1', is_admin: true })
    expect(s.needsAuth).toBe(false)
    expect(JSON.parse(window.localStorage.getItem(USER_KEY)!).id).toBe('u1')
  })

  it('drops a stale token whose whoami fails', async () => {
    vi.mocked(api.health).mockResolvedValue({ mode: 'server' })
    vi.mocked(getAuthToken).mockReturnValue('stale')
    vi.mocked(api.whoami).mockRejectedValue(new Error('401'))
    const s = useAuthStore()
    await s.init()
    expect(setAuthToken).toHaveBeenCalledWith(null)
    expect(s.user).toBe(null)
    expect(s.needsAuth).toBe(true)
  })

  it('resolves the run mode only once across concurrent callers', async () => {
    vi.mocked(api.health).mockResolvedValue({ mode: 'local' })
    const s = useAuthStore()
    await Promise.all([s.init(), s.init()])
    await s.init()
    expect(api.health).toHaveBeenCalledTimes(1)
  })
})

describe('auth store — login / register / logout', () => {
  it('stores the token and user on a successful login', async () => {
    vi.mocked(api.auth.login).mockResolvedValue({ token: 'newtok', user: { id: 'u1', is_admin: false } })
    const s = useAuthStore()
    const ok = await s.login('alice', 'password1')
    expect(ok).toBe(true)
    expect(setAuthToken).toHaveBeenCalledWith('newtok')
    expect(s.user).toEqual({ id: 'u1', is_admin: false })
    expect(s.error).toBe(null)
  })

  it('surfaces the backend error message on a failed login', async () => {
    vi.mocked(api.auth.login).mockRejectedValue(new Error('invalid username or password'))
    const s = useAuthStore()
    const ok = await s.login('alice', 'wrong')
    expect(ok).toBe(false)
    expect(s.error).toBe('invalid username or password')
    expect(s.user).toBe(null)
    expect(setAuthToken).not.toHaveBeenCalled()
  })

  it('registers and authenticates in one step', async () => {
    vi.mocked(api.auth.register).mockResolvedValue({ token: 'rtok', user: { id: 'admin', is_admin: true } })
    const s = useAuthStore()
    const ok = await s.register('admin', 'password1')
    expect(ok).toBe(true)
    expect(setAuthToken).toHaveBeenCalledWith('rtok')
    expect(s.user!.is_admin).toBe(true)
  })

  it('surfaces a duplicate-username conflict', async () => {
    vi.mocked(api.auth.register).mockRejectedValue(new Error('username already taken'))
    const s = useAuthStore()
    const ok = await s.register('taken', 'password1')
    expect(ok).toBe(false)
    expect(s.error).toBe('username already taken')
  })

  it('clears the session on logout even when the server call fails', async () => {
    vi.mocked(api.auth.logout).mockRejectedValue(new Error('offline'))
    const s = useAuthStore()
    s.user = { id: 'u1', is_admin: false }
    window.localStorage.setItem(USER_KEY, JSON.stringify(s.user))
    await s.logout()
    expect(setAuthToken).toHaveBeenCalledWith(null)
    expect(s.user).toBe(null)
    expect(window.localStorage.getItem(USER_KEY)).toBe(null)
  })
})
