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
    expect(text).toContain('(')
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
    expect(current.classes()).toContain('bg-yellow-200')
  })

  it('emits select with the clicked node id', async () => {
    const wrapper = mount(MoveTree, { props: { tree: sampleTree(), currentId: 0 } })
    await wrapper.findAll('[data-test="move"]')[2].trigger('click') // c5
    expect(wrapper.emitted('select')![0]).toEqual([3])
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
