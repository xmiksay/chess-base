// Keep the selected move visible in a scrolling move list.
//
// The math is a pure function of two rectangles so it can be unit-tested: jsdom
// reports zero layout for everything, so a component that simply called
// `el.scrollIntoView()` would be permanently untestable — and "the current move
// scrolls into view" is exactly the behaviour that kept regressing.

/** The scroll container: how far it is scrolled and how much it shows. */
export interface ScrollView {
  scrollTop: number
  clientHeight: number
}

/** The element to reveal, measured relative to the container's content box. */
export interface ScrollTarget {
  offsetTop: number
  offsetHeight: number
}

/**
 * The `scrollTop` that brings `target` into `view`, or null when it is already
 * fully visible (so the caller can skip the write and avoid a pointless reflow).
 *
 * `margin` keeps a little context above/below rather than parking the move flush
 * against an edge. A target taller than the viewport is aligned to its top —
 * scrolling to its bottom would hide the move itself.
 */
export function scrollOffsetFor(
  view: ScrollView,
  target: ScrollTarget,
  margin = 24,
): number | null {
  const viewTop = view.scrollTop
  const viewBottom = viewTop + view.clientHeight
  const targetTop = target.offsetTop
  const targetBottom = targetTop + target.offsetHeight

  if (targetTop >= viewTop && targetBottom <= viewBottom) return null

  if (target.offsetHeight >= view.clientHeight || targetTop < viewTop) {
    return Math.max(0, targetTop - margin)
  }
  return Math.max(0, targetBottom - view.clientHeight + margin)
}
