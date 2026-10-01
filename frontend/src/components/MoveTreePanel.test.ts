import { describe, it, expect, vi } from 'vitest'
import { mount } from '@vue/test-utils'
import MoveTreePanel from './MoveTreePanel.vue'
import type { MoveNode, MoveTree } from '../types'

function node(
  id: number,
  parent: number | null,
  san: string | null,
  children: number[],
  extra: Partial<MoveNode> = {},
): MoveNode {
  return { id, parent, san, comment: null, nags: [], children, ...extra }
}

/** 1.e4 e5 2.Nf3 with 1...c5 (2.Nc3 d6, and 2...Nc6 nested under it). */
function nested(): MoveTree {
  return {
    root: 0,
    nodes: [
      node(0, null, null, [1]),
      node(1, 0, 'e4', [2, 3]),
      node(2, 1, 'e5', [4]),
      node(3, 1, 'c5', [5], { comment: 'Sicilian' }),
      node(4, 2, 'Nf3', []),
      node(5, 3, 'Nc3', [6, 7]),
      node(6, 5, 'd6', []),
      node(7, 5, 'Nc6', [8, 9]),
      node(8, 7, 'Bb5', []),
      node(9, 7, 'g3', []),
    ],
  }
}

describe('MoveTreePanel', () => {
  it('renders one row per line, tagged with its depth', () => {
    const w = mount(MoveTreePanel, { props: { tree: nested(), currentId: 1 } })
    const depths = w.findAll('[data-test="move-row"]').map((r) => r.attributes('data-depth'))
    expect(depths).toEqual(['0', '1', '2', '3'])
  })

  // The defect: depth 0 was styled one way and *everything else* another, so a
  // depth-1 and a depth-5 variation were visually identical.
  it('styles each depth distinctly', () => {
    const w = mount(MoveTreePanel, { props: { tree: nested(), currentId: 1 } })
    const classes = w.findAll('[data-test="move-row"]').map((r) => r.attributes('class'))
    expect(new Set(classes).size).toBe(classes.length)
    expect(classes[1]).toContain('border-depth-1')
    expect(classes[2]).toContain('border-depth-2')
    expect(classes[3]).toContain('border-depth-3')
  })

  // Inverted from the old assertion: comments used to be a dot you had to click.
  it('renders comments inline, and a marker only when asked not to', () => {
    const inline = mount(MoveTreePanel, { props: { tree: nested(), currentId: 1 } })
    expect(inline.text()).toContain('Sicilian')
    expect(inline.find('[data-test="comment-marker"]').exists()).toBe(false)

    const marker = mount(MoveTreePanel, {
      props: { tree: nested(), currentId: 1, inlineComments: false },
    })
    expect(marker.text()).not.toContain('Sicilian')
    expect(marker.find('[data-test="comment-marker"]').exists()).toBe(true)
  })

  it('flags branch points so a variation is visible without clicking', () => {
    const w = mount(MoveTreePanel, { props: { tree: nested(), currentId: 1 } })
    expect(w.findAll('[data-test="branch-badge"]').length).toBeGreaterThan(0)
  })

  it('bounds its own height and scrolls in place', () => {
    const w = mount(MoveTreePanel, { props: { tree: nested(), currentId: 1 } })
    expect(w.find('[data-test="move-tree"]').classes()).toContain('overflow-y-auto')
  })

  it('highlights and selects', async () => {
    const w = mount(MoveTreePanel, { props: { tree: nested(), currentId: 1 } })
    const moves = w.findAll('[data-test="move"]')
    expect(moves[0].classes()).toContain('ring-accent')
    await moves[1].trigger('click')
    expect(w.emitted('select')![0]).toEqual([2])
  })

  it('prompts to start a line for an empty tree', () => {
    const empty: MoveTree = { root: 0, nodes: [node(0, null, null, [])] }
    const w = mount(MoveTreePanel, { props: { tree: empty, currentId: 0 } })
    expect(w.text()).toContain('No moves yet')
  })

  describe('node actions', () => {
    // The core regression: nothing but moves may live in the move flow, or
    // selecting one reflows the rest under the cursor and the next click lands
    // on the previous move's delete.
    it('puts no action controls in the move flow', () => {
      const w = mount(MoveTreePanel, { props: { tree: nested(), currentId: 5, editable: true } })
      for (const row of w.findAll('[data-test="move-row"]')) {
        expect(row.findAll('button').length).toBe(row.findAll('[data-test="move"]').length)
      }
      expect(w.find('[data-test="node-menu"]').exists()).toBe(false)
    })

    it('opens the menu on right-click and selects what was clicked', async () => {
      const w = mount(MoveTreePanel, {
        props: { tree: nested(), currentId: 1, editable: true },
        attachTo: document.body,
      })
      await w.findAll('[data-test="move"]')[4].trigger('contextmenu')
      expect(w.emitted('select')!.at(-1)).toEqual([5])
      expect(w.find('[data-test="node-menu"]').text()).toContain('Nc3')
      w.unmount()
    })

    it('arms delete only after a moment, then emits remove', async () => {
      vi.useFakeTimers()
      const w = mount(MoveTreePanel, {
        props: { tree: nested(), currentId: 1, editable: true },
        attachTo: document.body,
      })
      await w.findAll('[data-test="move"]')[4].trigger('contextmenu')

      const del = w.find('[data-test="menu-delete"]')
      expect(del.attributes('disabled')).toBeDefined()

      vi.advanceTimersByTime(300)
      await w.vm.$nextTick()
      await w.find('[data-test="menu-delete"]').trigger('click')
      expect(w.emitted('remove')![0]).toEqual([5])
      w.unmount()
      vi.useRealTimers()
    })

    it('offers no edit actions when read-only', async () => {
      const w = mount(MoveTreePanel, {
        props: { tree: nested(), currentId: 1 },
        attachTo: document.body,
      })
      await w.findAll('[data-test="move"]')[0].trigger('contextmenu')
      expect(w.find('[data-test="menu-delete"]').exists()).toBe(false)
      expect(w.find('[data-test="menu-promote"]').exists()).toBe(false)
      w.unmount()
    })

    it('disables promote for a node already first among its siblings', async () => {
      const w = mount(MoveTreePanel, {
        props: { tree: nested(), currentId: 1, editable: true },
        attachTo: document.body,
      })
      // 1.e4 is its parent's only child — nowhere to promote to.
      await w.findAll('[data-test="move"]')[0].trigger('contextmenu')
      expect(w.find('[data-test="menu-promote"]').attributes('disabled')).toBeDefined()
      w.unmount()
    })

    it('hides the copy actions until a host wires them', async () => {
      const w = mount(MoveTreePanel, {
        props: { tree: nested(), currentId: 1, editable: true },
        attachTo: document.body,
      })
      await w.findAll('[data-test="move"]')[0].trigger('contextmenu')
      expect(w.find('[data-test="menu-copy-fen"]').exists()).toBe(false)
      w.unmount()
    })

    it('closes on Escape', async () => {
      const w = mount(MoveTreePanel, {
        props: { tree: nested(), currentId: 1, editable: true },
        attachTo: document.body,
      })
      await w.findAll('[data-test="move"]')[0].trigger('contextmenu')
      expect(w.find('[data-test="node-menu"]').exists()).toBe(true)

      window.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape' }))
      await w.vm.$nextTick()
      expect(w.find('[data-test="node-menu"]').exists()).toBe(false)
      w.unmount()
    })
  })
})
