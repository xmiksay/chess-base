import { describe, it, expect } from 'vitest'
import { mount } from '@vue/test-utils'
import MoveComment from './MoveComment.vue'

function sampleTree() {
  return {
    root: 0,
    nodes: [
      { id: 0, parent: null, san: null, comment: null, nags: [], children: [1] },
      { id: 1, parent: 0, san: 'e4', comment: null, nags: [], children: [2] },
      { id: 2, parent: 1, san: 'c5', comment: 'Sicilian', nags: [5], children: [] },
    ],
  }
}

describe('MoveComment', () => {
  it('shows the selected move comment with its SAN and NAG', () => {
    const wrapper = mount(MoveComment, { props: { tree: sampleTree(), currentId: 2 } })
    const text = wrapper.text()
    expect(text).toContain('Sicilian')
    expect(text).toContain('c5')
    expect(text).toContain('!?') // NAG 5
  })

  it('shows a placeholder for a move without a comment', () => {
    const wrapper = mount(MoveComment, { props: { tree: sampleTree(), currentId: 1 } })
    expect(wrapper.text()).toContain('No comment on this move.')
  })

  it('shows a placeholder at the root and when nothing is selected', () => {
    const atRoot = mount(MoveComment, { props: { tree: sampleTree(), currentId: 0 } })
    expect(atRoot.text()).toContain('No comment on this move.')
    const none = mount(MoveComment, { props: { tree: null, currentId: null } })
    expect(none.text()).toContain('No comment on this move.')
  })
})
