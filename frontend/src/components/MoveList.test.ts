import { describe, it, expect } from 'vitest'
import { mount } from '@vue/test-utils'
import MoveList from './MoveList.vue'

const history = ['e4', 'e5', 'Nf3', 'Nc6']

describe('MoveList', () => {
  it('renders moves as numbered pairs', () => {
    const wrapper = mount(MoveList, { props: { history, currentPly: 0 } })
    const text = wrapper.text()
    expect(text).toContain('1.')
    expect(text).toContain('e4')
    expect(text).toContain('e5')
    expect(text).toContain('2.')
    expect(text).toContain('Nf3')
    expect(wrapper.findAll('[data-test="move"]')).toHaveLength(4)
  })

  it('highlights the move at the current ply', () => {
    const wrapper = mount(MoveList, { props: { history, currentPly: 3 } }) // after Nf3
    const moves = wrapper.findAll('[data-test="move"]')
    expect(moves[2].classes()).toContain('bg-yellow-200') // Nf3 is ply 3
    expect(moves[0].classes()).not.toContain('bg-yellow-200')
  })

  it('emits select with the clicked move ply', async () => {
    const wrapper = mount(MoveList, { props: { history, currentPly: 0 } })
    await wrapper.findAll('[data-test="move"]')[3].trigger('click') // Nc6 is ply 4
    expect(wrapper.emitted('select')![0]).toEqual([4])
  })

  it('prompts when there are no moves', () => {
    const wrapper = mount(MoveList, { props: { history: [], currentPly: 0 } })
    expect(wrapper.text()).toContain('No moves yet')
    expect(wrapper.findAll('[data-test="move"]')).toHaveLength(0)
  })
})
