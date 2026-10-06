import { describe, it, expect } from 'vitest'
import { defineComponent, h, ref, nextTick } from 'vue'
import { mount } from '@vue/test-utils'
import { useScrollIntoView } from './useScrollIntoView'

/**
 * jsdom reports zero for every layout property, so the container and its rows
 * are given explicit geometry. That is the whole reason the arithmetic lives in
 * the pure `scrollOffsetFor` and this composable stays a thin DOM shim.
 */
function harness(opts: { suppress?: () => boolean } = {}) {
  const currentId = ref<number | null>(1)
  const container = ref<HTMLElement | null>(null)

  const Host = defineComponent({
    setup() {
      useScrollIntoView({ container, currentId, suppress: opts.suppress })
      return () =>
        h('div', { ref: container }, [1, 2, 3].map((id) => h('button', { 'data-node-id': id }, String(id))))
    },
  })

  const w = mount(Host, { attachTo: document.body })
  const host = container.value!
  Object.defineProperty(host, 'clientHeight', { value: 100, configurable: true })
  Object.defineProperty(host, 'offsetTop', { value: 0, configurable: true })
  host.scrollTop = 0
  for (const [i, el] of [...host.querySelectorAll('button')].entries()) {
    Object.defineProperty(el, 'offsetTop', { value: i * 200, configurable: true })
    Object.defineProperty(el, 'offsetHeight', { value: 20, configurable: true })
  }
  return { w, host, currentId }
}

describe('useScrollIntoView', () => {
  it('scrolls the selected move into view', async () => {
    const { host, currentId } = harness()
    expect(host.scrollTop).toBe(0)

    currentId.value = 3 // offsetTop 400, viewport 0..100
    await nextTick()
    await nextTick()
    expect(host.scrollTop).toBeGreaterThan(0)
  })

  it('leaves the scroll alone when the move is already visible', async () => {
    const { host, currentId } = harness()
    currentId.value = 1 // offsetTop 0 — inside 0..100
    await nextTick()
    await nextTick()
    expect(host.scrollTop).toBe(0)
  })

  // Yanking the list out from under the cursor right after a click is worse
  // than leaving the selection where the user put it.
  it('does not scroll when the selection came from a click in the panel', async () => {
    let clicked = true
    const { host, currentId } = harness({ suppress: () => clicked })
    currentId.value = 3
    await nextTick()
    await nextTick()
    expect(host.scrollTop).toBe(0)

    clicked = false
    currentId.value = 2
    await nextTick()
    await nextTick()
    expect(host.scrollTop).toBeGreaterThan(0)
  })

  it('ignores a selection with no rendered row', async () => {
    const { host, currentId } = harness()
    currentId.value = 99
    await nextTick()
    await nextTick()
    expect(host.scrollTop).toBe(0)
  })
})
