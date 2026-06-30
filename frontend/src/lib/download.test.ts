import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest'
import { downloadText } from './download'

describe('downloadText', () => {
  beforeEach(() => {
    // jsdom has no object-URL plumbing; stub it so the trigger is observable.
    vi.stubGlobal('URL', {
      createObjectURL: vi.fn(() => 'blob:mock'),
      revokeObjectURL: vi.fn(),
    })
  })

  afterEach(() => {
    vi.unstubAllGlobals()
    vi.restoreAllMocks()
  })

  it('creates an anchor with the filename and clicks it', () => {
    const click = vi.spyOn(HTMLAnchorElement.prototype, 'click').mockImplementation(() => {})

    downloadText('study-7.pgn', '1. e4 e5 *')

    expect(URL.createObjectURL).toHaveBeenCalledTimes(1)
    const blob = vi.mocked(URL.createObjectURL).mock.calls[0][0] as Blob
    expect(blob.type).toBe('application/x-chess-pgn')
    expect(click).toHaveBeenCalledTimes(1)
    // The transient anchor is cleaned up and the object URL revoked.
    expect(document.querySelector('a')).toBeNull()
    expect(URL.revokeObjectURL).toHaveBeenCalledWith('blob:mock')
  })
})
