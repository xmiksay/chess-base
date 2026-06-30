import { describe, it, expect, beforeEach, vi } from 'vitest'
import { mount, flushPromises } from '@vue/test-utils'
import { setActivePinia, createPinia } from 'pinia'
import GenerateDangerMapDialog from './GenerateDangerMapDialog.vue'
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

const view = {
  id: 9,
  database_id: 2,
  name: 'Smith-Morra',
  global: false,
  node_count: 12,
  rejected: 1,
  roles: [{ node_id: 3, san: 'Nxe4', kind: 'Trap', role: 'Weapon' }],
}

describe('GenerateDangerMapDialog', () => {
  beforeEach(() => {
    setActivePinia(createPinia())
    vi.clearAllMocks()
  })

  it('disables submit and shows the hint when llm is unavailable', async () => {
    const wrapper = mount(GenerateDangerMapDialog, { props: { llmEnabled: false } })
    await flushPromises()
    expect(wrapper.find('[data-test="submit"]').attributes('disabled')).toBeDefined()
    expect(wrapper.find('[data-test="llm-hint"]').exists()).toBe(true)
  })

  it('keeps submit disabled until both a name and a spine PGN are given', async () => {
    const wrapper = mount(GenerateDangerMapDialog, { props: { llmEnabled: true } })
    await flushPromises()
    await wrapper.find('[data-test="name"]').setValue('Smith-Morra')
    // Name alone is not enough — the spine is required.
    expect(wrapper.find('[data-test="submit"]').attributes('disabled')).toBeDefined()
    await wrapper.find('[data-test="spine-pgn"]').setValue('1. e4 c5 *')
    expect(wrapper.find('[data-test="submit"]').attributes('disabled')).toBeUndefined()
  })

  it('submits with the right body shape and lists the danger roles', async () => {
    const studies = useStudiesStore()
    const generate = vi.spyOn(studies, 'generateDangerMap').mockResolvedValue(view)

    const wrapper = mount(GenerateDangerMapDialog, { props: { llmEnabled: true } })
    await flushPromises()

    await wrapper.find('[data-test="name"]').setValue('Smith-Morra')
    await wrapper.find('[data-test="spine-pgn"]').setValue('1. e4 c5 2. d4 *')
    await wrapper.find('[data-test="our-side"]').setValue('Black')
    await wrapper.find('[data-test="max-depth"]').setValue(10)
    await wrapper.find('[data-test="movetime"]').setValue(800)
    await wrapper.find('[data-test="multipv"]').setValue(3)
    await wrapper.find('form').trigger('submit')
    await flushPromises()

    expect(generate).toHaveBeenCalledWith({
      database_id: 2,
      name: 'Smith-Morra',
      spine_pgn: '1. e4 c5 2. d4 *',
      start_fen: STARTPOS_FEN,
      spine: { our_side: 'Black', max_depth: 10 },
      movetime_ms: 800,
      multipv: 3,
    })
    expect(wrapper.find('[data-test="roles"]').text()).toContain('Nxe4')
    expect(wrapper.find('[data-test="open-result"]').exists()).toBe(true)
  })

  it('emits open with the generated study id', async () => {
    const studies = useStudiesStore()
    vi.spyOn(studies, 'generateDangerMap').mockResolvedValue(view)
    const wrapper = mount(GenerateDangerMapDialog, { props: { llmEnabled: true } })
    await flushPromises()
    await wrapper.find('[data-test="name"]').setValue('Smith-Morra')
    await wrapper.find('[data-test="spine-pgn"]').setValue('1. e4 c5 *')
    await wrapper.find('form').trigger('submit')
    await flushPromises()
    await wrapper.find('[data-test="open-result"]').trigger('click')
    expect(wrapper.emitted('open')![0]).toEqual([9])
  })
})
