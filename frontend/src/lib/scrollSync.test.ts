import { describe, it, expect } from 'vitest'
import { scrollOffsetFor } from './scrollSync'

const view = { scrollTop: 100, clientHeight: 200 } // shows 100..300

describe('scrollOffsetFor', () => {
  it('returns null when the target is already fully visible', () => {
    expect(scrollOffsetFor(view, { offsetTop: 150, offsetHeight: 20 })).toBeNull()
  })

  it('scrolls up to reveal a target above the viewport', () => {
    expect(scrollOffsetFor(view, { offsetTop: 40, offsetHeight: 20 })).toBe(16) // 40 - 24
  })

  it('scrolls down to reveal a target below the viewport', () => {
    // bottom 420 - height 200 + margin 24
    expect(scrollOffsetFor(view, { offsetTop: 400, offsetHeight: 20 })).toBe(244)
  })

  it('honours the margin', () => {
    expect(scrollOffsetFor(view, { offsetTop: 40, offsetHeight: 20 }, 0)).toBe(40)
  })

  it('never scrolls above the top of the content', () => {
    expect(scrollOffsetFor(view, { offsetTop: 5, offsetHeight: 10 })).toBe(0)
  })

  it('aligns a target taller than the viewport to its top', () => {
    // Scrolling to its bottom would push the move itself out of sight.
    expect(scrollOffsetFor(view, { offsetTop: 500, offsetHeight: 400 })).toBe(476)
  })

  it('treats a target flush with an edge as visible', () => {
    expect(scrollOffsetFor(view, { offsetTop: 100, offsetHeight: 200 })).toBeNull()
  })
})
