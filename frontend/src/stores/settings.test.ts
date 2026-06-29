import { describe, it, expect, vi, beforeEach } from 'vitest'
import { setActivePinia, createPinia } from 'pinia'
import { useSettingsStore } from './settings'
import { api } from '../api'

vi.mock('../api', () => ({
  api: {
    settings: { get: vi.fn(), set: vi.fn() },
  },
}))

const STORAGE_KEY = 'chess-base:settings'

beforeEach(() => {
  vi.clearAllMocks()
  window.localStorage.clear()
  setActivePinia(createPinia())
})

describe('settings store', () => {
  it('starts from defaults when nothing is mirrored', () => {
    const s = useSettingsStore()
    expect(s.theme).toBe('system')
    expect(s.boardTheme).toBe('brown')
    expect(s.defaultDatabaseId).toBe(null)
  })

  it('hydrates from the localStorage mirror instantly (before any fetch)', () => {
    window.localStorage.setItem(
      STORAGE_KEY,
      JSON.stringify({ theme: 'dark', boardTheme: 'green', defaultDatabaseId: 7 }),
    )
    const s = useSettingsStore()
    expect(s.theme).toBe('dark')
    expect(s.boardTheme).toBe('green')
    expect(s.defaultDatabaseId).toBe(7)
  })

  it('load() reconciles with the server and maps snake_case', async () => {
    vi.mocked(api.settings.get).mockResolvedValue({
      theme: 'light',
      board_theme: 'blue',
      default_database_id: 3,
    })
    const s = useSettingsStore()
    await s.load()
    expect(s.theme).toBe('light')
    expect(s.boardTheme).toBe('blue')
    expect(s.defaultDatabaseId).toBe(3)
    // The mirror is refreshed from the server response.
    expect(JSON.parse(window.localStorage.getItem(STORAGE_KEY)!).theme).toBe('light')
  })

  it('load() keeps the mirror when the server is unreachable', async () => {
    window.localStorage.setItem(STORAGE_KEY, JSON.stringify({ theme: 'dark' }))
    vi.mocked(api.settings.get).mockRejectedValue(new Error('offline'))
    const s = useSettingsStore()
    await s.load()
    expect(s.theme).toBe('dark')
    expect(s.error).toContain('offline')
  })

  it('update() applies optimistically, mirrors, and persists snake_case', async () => {
    vi.mocked(api.settings.set).mockResolvedValue({ theme: 'dark', board_theme: 'green' })
    const s = useSettingsStore()
    await s.update({ boardTheme: 'green', theme: 'dark' })

    expect(api.settings.set).toHaveBeenCalledWith(
      expect.objectContaining({ theme: 'dark', board_theme: 'green' }),
    )
    expect(s.boardTheme).toBe('green')
    expect(JSON.parse(window.localStorage.getItem(STORAGE_KEY)!).boardTheme).toBe('green')
  })

  it('update() surfaces a rejected save as an error but keeps the optimistic value', async () => {
    vi.mocked(api.settings.set).mockRejectedValue(new Error('database 9 is not available'))
    const s = useSettingsStore()
    await s.update({ defaultDatabaseId: 9 })
    expect(s.defaultDatabaseId).toBe(9) // optimistic value retained
    expect(s.error).toContain('not available')
  })

  it('applyTheme() toggles the document dark class', () => {
    const s = useSettingsStore()
    s.theme = 'dark'
    s.applyTheme()
    expect(document.documentElement.classList.contains('dark')).toBe(true)
    s.theme = 'light'
    s.applyTheme()
    expect(document.documentElement.classList.contains('dark')).toBe(false)
  })
})
