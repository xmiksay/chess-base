import { describe, it, expect, beforeEach, vi } from 'vitest'
import { mount, flushPromises } from '@vue/test-utils'
import { setActivePinia, createPinia } from 'pinia'
import GenerateStudyDialog from './GenerateStudyDialog.vue'
import { STARTPOS_FEN } from '../lib/fen'

// Stub the API so the dialog can list databases without a network call.
vi.mock('../api', () => ({
  api: {
    databases: {
      list: vi.fn().mockResolvedValue([
        { id: 2, owner_id: null, name: 'Repertoire', kind: 'own', index_depth: null, global: false },
      ]),
    },
  },
}))

import { useStudiesStore } from '../stores/studies'

describe('GenerateStudyDialog', () => {
  beforeEach(() => {
    setActivePinia(createPinia())
    vi.clearAllMocks()
  })

  it('disables submit and shows the hint when llm is unavailable', async () => {
    const wrapper = mount(GenerateStudyDialog, { props: { llmEnabled: false } })
    await flushPromises()
    expect(wrapper.find('[data-test="submit"]').attributes('disabled')).toBeDefined()
    expect(wrapper.find('[data-test="llm-hint"]').exists()).toBe(true)
  })

  it('submits with the right body shape when llm is enabled', async () => {
    const studies = useStudiesStore()
    const generate = vi
      .spyOn(studies, 'generate')
      .mockResolvedValue({ id: 9, database_id: 2, name: 'Najdorf', global: false, node_count: 12, rejected: 1 })

    const wrapper = mount(GenerateStudyDialog, { props: { llmEnabled: true } })
    await flushPromises()

    await wrapper.find('[data-test="name"]').setValue('Najdorf')
    await wrapper.find('[data-test="engine-depth"]').setValue(20)
    await wrapper.find('[data-test="max-depth"]').setValue(8)
    await wrapper.find('[data-test="max-children"]').setValue(4)
    await wrapper.find('form').trigger('submit')
    await flushPromises()

    expect(generate).toHaveBeenCalledWith({
      database_id: 2,
      name: 'Najdorf',
      start_fen: STARTPOS_FEN,
      engine_depth: 20,
      tree: { max_depth: 8, max_children: 4 },
      plan_lines: 0,
      threats: false,
    })
    // Result panel offers to open the generated study.
    expect(wrapper.find('[data-test="open-result"]').exists()).toBe(true)
  })

  it('submits the plan-lines count and threats toggle', async () => {
    const studies = useStudiesStore()
    const generate = vi
      .spyOn(studies, 'generate')
      .mockResolvedValue({ id: 9, database_id: 2, name: 'Najdorf', global: false, node_count: 12, rejected: 0 })

    const wrapper = mount(GenerateStudyDialog, { props: { llmEnabled: true } })
    await flushPromises()

    await wrapper.find('[data-test="name"]').setValue('Najdorf')
    await wrapper.find('[data-test="plan-lines"]').setValue(2)
    await wrapper.find('[data-test="threats"]').setValue(true)
    await wrapper.find('form').trigger('submit')
    await flushPromises()

    expect(generate).toHaveBeenCalledWith(
      expect.objectContaining({ plan_lines: 2, threats: true }),
    )
  })

  it('emits open with the generated study id', async () => {
    const studies = useStudiesStore()
    vi.spyOn(studies, 'generate').mockResolvedValue({
      id: 9,
      database_id: 2,
      name: 'Najdorf',
      global: false,
      node_count: 12,
      rejected: 1,
    })
    const wrapper = mount(GenerateStudyDialog, { props: { llmEnabled: true } })
    await flushPromises()
    await wrapper.find('[data-test="name"]').setValue('Najdorf')
    await wrapper.find('form').trigger('submit')
    await flushPromises()
    await wrapper.find('[data-test="open-result"]').trigger('click')
    expect(wrapper.emitted('open')![0]).toEqual([9])
  })
})
