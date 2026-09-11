import { describe, it, expect } from 'vitest'
import { mount } from '@vue/test-utils'
import MoveTree from './MoveTree.vue'

function sampleTree() {
  return {
    root: 0,
    nodes: [
      { id: 0, parent: null, san: null, comment: null, nags: [], children: [1] },
      { id: 1, parent: 0, san: 'e4', comment: null, nags: [], children: [2, 3] },
      { id: 2, parent: 1, san: 'e5', comment: null, nags: [], children: [] },
      { id: 3, parent: 1, san: 'c5', comment: 'Sicilian', nags: [5], children: [] },
    ],
  }
}

describe('MoveTree', () => {
  it('renders mainline moves, a bracketed variation, a comment marker and NAG', () => {
    const wrapper = mount(MoveTree, { props: { tree: sampleTree(), currentId: 1 } })
    const text = wrapper.text()
    expect(text).toContain('1.e4')
    expect(text).toContain('e5')
    // The c5 line is a variation: it renders as its own indented block (no more
    // inline parentheses), so the tree carries a variation container.
    expect(wrapper.findAll('[data-test="variation"]')).toHaveLength(1)
    expect(text).toContain('c5')
    expect(text).toContain('!?') // NAG 5
    // The comment text is shown in MoveComment, not inline; the list only marks
    // commented moves with a single dot (and never the literal text).
    expect(text).not.toContain('Sicilian')
    expect(wrapper.findAll('[data-test="comment-marker"]')).toHaveLength(1)
    expect(wrapper.findAll('[data-test="move"]')).toHaveLength(3)
  })

  it('highlights the current node', () => {
    const wrapper = mount(MoveTree, { props: { tree: sampleTree(), currentId: 1 } })
    const current = wrapper.findAll('[data-test="move"]')[0]
    expect(current.classes()).toContain('ring-accent')
  })

  it('emits select with the clicked node id', async () => {
    const wrapper = mount(MoveTree, { props: { tree: sampleTree(), currentId: 0 } })
    await wrapper.findAll('[data-test="move"]')[2].trigger('click') // c5
    expect(wrapper.emitted('select')![0]).toEqual([3])
  })

  // Regression: the promote/demote/delete toolbar used to render inline, right
  // after the selected move, so selecting a move reflowed the rest of the line
  // to the right and the next click landed on the previous move's ✕. The actions
  // must never sit inside the move flow — at any depth.
  it('keeps node actions out of the move flow', () => {
    const deep = {
      root: 0,
      nodes: [
        { id: 0, parent: null, san: null, comment: null, nags: [], children: [1] },
        { id: 1, parent: 0, san: 'e4', comment: null, nags: [], children: [2, 3] },
        { id: 2, parent: 1, san: 'e5', comment: null, nags: [], children: [] },
        { id: 3, parent: 1, san: 'c5', comment: null, nags: [], children: [4] },
        { id: 4, parent: 3, san: 'Nf3', comment: null, nags: [], children: [5] },
        { id: 5, parent: 4, san: 'd6', comment: null, nags: [], children: [] },
      ],
    }
    for (const currentId of [1, 3, 4, 5]) {
      const wrapper = mount(MoveTree, { props: { tree: deep, currentId, editable: true } })
      const flow = wrapper.find('[data-test="move-flow"]')
      // Nothing clickable other than the moves themselves lives in the flow, so
      // selecting a move can never shift the next one under the cursor.
      expect(flow.findAll('button')).toHaveLength(wrapper.findAll('[data-test="move"]').length)
      expect(flow.find('[data-test="node-actions"]').exists()).toBe(false)
      // They exist — just hoisted out of the flow, acting on the selection.
      expect(wrapper.find('[data-test="node-actions"]').exists()).toBe(true)
    }
  })

  it('acts on the selected node from the hoisted toolbar', async () => {
    const wrapper = mount(MoveTree, {
      props: { tree: sampleTree(), currentId: 3, editable: true },
    })
    expect(wrapper.find('[data-test="node-actions"]').text()).toContain('c5')
    await wrapper.find('[data-test="node-delete"]').trigger('click')
    expect(wrapper.emitted('remove')![0]).toEqual([3])
    await wrapper.find('[data-test="node-promote"]').trigger('click')
    expect(wrapper.emitted('promote')![0]).toEqual([3])
  })

  it('offers no node actions when not editable, or at the root', () => {
    const readOnly = mount(MoveTree, { props: { tree: sampleTree(), currentId: 3 } })
    expect(readOnly.find('[data-test="node-actions"]').exists()).toBe(false)
    // The root has no move to promote or delete.
    const atRoot = mount(MoveTree, {
      props: { tree: sampleTree(), currentId: 0, editable: true },
    })
    expect(atRoot.find('[data-test="node-actions"]').exists()).toBe(false)
  })

  it('prompts to start a line for an empty tree', () => {
    const empty = {
      root: 0,
      nodes: [{ id: 0, parent: null, san: null, comment: null, nags: [], children: [] }],
    }
    const wrapper = mount(MoveTree, { props: { tree: empty, currentId: 0 } })
    expect(wrapper.text()).toContain('No moves yet')
  })
})
