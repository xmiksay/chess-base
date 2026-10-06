// Keep the selected move visible as the cursor moves.
//
// Thin on purpose: all the arithmetic is `scrollSync.scrollOffsetFor`, which is
// pure and unit-tested. This only does the DOM half — find the element, read two
// rectangles, assign `scrollTop` — because jsdom reports zero layout and so
// cannot exercise any of it.

import { nextTick, watch, type Ref } from 'vue'
import { scrollOffsetFor } from './scrollSync'

export interface ScrollIntoViewOptions {
  /** The scrolling container holding the move buttons. */
  container: Ref<HTMLElement | null>
  /** The node id to reveal; null disables. */
  currentId: Ref<number | null | undefined>
  /**
   * Suppress the scroll for this update — pass true when the user clicked a
   * move in the panel. Yanking the text out from under their cursor right after
   * they clicked it is worse than leaving the selection where it is.
   */
  suppress?: () => boolean
  margin?: number
}

export function useScrollIntoView(opts: ScrollIntoViewOptions): void {
  watch(
    () => opts.currentId.value,
    async (id) => {
      if (id == null) return
      if (opts.suppress?.()) return
      // Wait for the row to exist: selection and render land in the same tick.
      await nextTick()
      const host = opts.container.value
      if (!host) return
      const target = host.querySelector<HTMLElement>(`[data-node-id="${id}"]`)
      if (!target) return

      const next = scrollOffsetFor(
        { scrollTop: host.scrollTop, clientHeight: host.clientHeight },
        { offsetTop: target.offsetTop - host.offsetTop, offsetHeight: target.offsetHeight },
        opts.margin,
      )
      if (next !== null) host.scrollTop = next
    },
    { flush: 'post' },
  )
}
