import { describe, it, expect } from 'vitest'
import { mount } from '@vue/test-utils'
import EvalGraph from './EvalGraph.vue'
import type { MoveReview } from '../types'

function sampleMoves(): MoveReview[] {
  return [
    { ply: 1, san: 'e4', eval_cp: 20, classification: 'best', explanation: '' },
    { ply: 2, san: 'e5', eval_cp: -1500, classification: 'good', explanation: '' },
    { ply: 3, san: 'Nf3', eval_cp: 60, classification: 'good', explanation: '' },
  ]
}

describe('EvalGraph', () => {
  it('renders one point per reviewed move', () => {
    const wrapper = mount(EvalGraph, { props: { moves: sampleMoves(), currentPly: 1 } })
    expect(wrapper.findAll('[data-test="eval-point"]')).toHaveLength(3)
  })

  it('emits select with the ply of the clicked point', async () => {
    const wrapper = mount(EvalGraph, { props: { moves: sampleMoves(), currentPly: 1 } })
    await wrapper.findAll('[data-test="eval-point"]')[2].trigger('click')
    expect(wrapper.emitted('select')![0]).toEqual([3])
  })

  it('renders an empty graph with no points', () => {
    const wrapper = mount(EvalGraph, { props: { moves: [], currentPly: 0 } })
    expect(wrapper.findAll('[data-test="eval-point"]')).toHaveLength(0)
    expect(wrapper.find('[data-test="eval-graph"]').exists()).toBe(true)
  })
})
