import { describe, it, expect, beforeEach, vi } from 'vitest'
import { setActivePinia, createPinia } from 'pinia'
import { useBoardOverlays } from './useBoardOverlays'
import { useSettingsStore } from '../stores/settings'
import { useOverlaysStore } from '../stores/overlays'

vi.mock('../api', () => ({
  api: {
    settings: { set: vi.fn().mockResolvedValue({}) },
    search: { threats: vi.fn().mockResolvedValue([]), tree: vi.fn().mockResolvedValue([]) },
  },
}))

beforeEach(() => {
  window.localStorage.clear()
  setActivePinia(createPinia())
})

describe('useBoardOverlays clear (issue #190)', () => {
  it('turns every layer toggle off and empties boardShapes', () => {
    const settings = useSettingsStore()
    const overlays = useOverlaysStore()
    const { boardShapes, clear } = useBoardOverlays(() => 'fen')

    settings.showThreats = true
    overlays.threats = [{ orig: 'd6', dest: 'e5', brush: 'threat' }]
    expect(boardShapes.value).toEqual([{ orig: 'd6', dest: 'e5', brush: 'threat' }])

    clear()

    expect(settings.showPlans).toBe(false)
    expect(settings.showThreats).toBe(false)
    expect(settings.showMasterMoves).toBe(false)
    expect(overlays.threats).toEqual([])
    expect(overlays.master).toEqual([])
    expect(boardShapes.value).toEqual([])
  })

  it('stays off across a position change until a layer is re-enabled', () => {
    const settings = useSettingsStore()
    const overlays = useOverlaysStore()
    let fen = 'fen-1'
    const { boardShapes, clear } = useBoardOverlays(() => fen)

    settings.showMasterMoves = true
    overlays.master = [{ orig: 'e2', dest: 'e4', brush: 'master' }]
    clear()
    expect(boardShapes.value).toEqual([])

    // Navigating to a new position must not resurrect the cleared layer.
    fen = 'fen-2'
    overlays.master = [{ orig: 'g1', dest: 'f3', brush: 'master' }]
    expect(boardShapes.value).toEqual([])
  })
})
