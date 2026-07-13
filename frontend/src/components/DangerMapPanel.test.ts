import { describe, it, expect, beforeEach, vi } from 'vitest'
import { flushPromises, mount } from '@vue/test-utils'
import { setActivePinia, createPinia } from 'pinia'

// Mock the API client: the panel drives studies.dangerMap/mergeDanger/get.
vi.mock('../api', () => ({
  api: {
    studies: {
      dangerMap: vi.fn(),
      mergeDanger: vi.fn(),
      get: vi.fn(),
    },
  },
}))

import { api } from '../api'
import DangerMapPanel from './DangerMapPanel.vue'
import type { DangerTree, MergeDangerResult, Study } from '../types'

const study: Study = {
  id: 5,
  database_id: 1,
  name: 'Repertoire',
  global: false,
  owner_id: 'bob',
  folder_id: null,
  origin_game_id: null,
  tree: { root: 0, nodes: [] },
}

const walkedTree: DangerTree = {
  root: 0,
  nodes: [
    { id: 0, parent: null, fen: 'startpos', ply: 0, children: [1] },
    {
      id: 1,
      parent: 0,
      san: 'Qh5',
      fen: 'after-qh5',
      ply: 1,
      children: [],
      tag: {
        kind: 'Trap',
        role: 'Weapon',
        trap: 'Weapon',
        eval: { cp: 30 },
      },
    },
  ],
}

async function setup() {
  const wrapper = mount(DangerMapPanel, { props: { engineEnabled: true, studyId: 5 } })
  await wrapper.find('[data-test="danger-spine"]').setValue('1. e4 *')
  await wrapper.find('[data-test="danger-show"]').trigger('click')
  await flushPromises()
  return wrapper
}

describe('DangerMapPanel', () => {
  beforeEach(() => {
    setActivePinia(createPinia())
    vi.clearAllMocks()
    vi.mocked(api.studies.dangerMap).mockResolvedValue({ tree: walkedTree, roles: [] })
    vi.mocked(api.studies.get).mockResolvedValue(study)
  })

  it('shows the eval on a tagged role row', async () => {
    const wrapper = await setup()
    const row = wrapper.find('[data-test="danger-role"]')
    expect(row.text()).toContain('+0.30')
  })

  it('reports how many nodes and roles a graft actually added', async () => {
    const merged: MergeDangerResult = { ...study, added_nodes: 3, weapons: 2, cautions: 1 }
    vi.mocked(api.studies.mergeDanger).mockResolvedValue(merged)

    const wrapper = await setup()
    await wrapper.find('[data-test="danger-extend"]').trigger('click')
    await flushPromises()

    expect(wrapper.find('[data-test="danger-merge-summary"]').text()).toBe(
      '3 new nodes, 2 Weapons, 1 Caution',
    )
  })

  it('reports no new lines on an idempotent re-merge', async () => {
    const merged: MergeDangerResult = { ...study, added_nodes: 0, weapons: 0, cautions: 0 }
    vi.mocked(api.studies.mergeDanger).mockResolvedValue(merged)

    const wrapper = await setup()
    await wrapper.find('[data-test="danger-extend"]').trigger('click')
    await flushPromises()

    expect(wrapper.find('[data-test="danger-merge-summary"]').text()).toBe(
      'No new lines — already merged.',
    )
  })
})
