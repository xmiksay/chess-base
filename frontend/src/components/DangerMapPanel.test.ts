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
  public: false,
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

  it('prefills our_side from the side to move at the start position (issue #194)', async () => {
    // Startpos (White to move) defaults to White...
    const white = mount(DangerMapPanel, { props: { engineEnabled: true, studyId: 5 } })
    expect(
      (white.find('[data-test="danger-side"]').element as HTMLSelectElement).value,
    ).toBe('White')

    // ...while a start FEN with Black to move (e.g. a Black repertoire's
    // opening spot after the opponent's first move) defaults to Black.
    const black = mount(DangerMapPanel, {
      props: {
        engineEnabled: true,
        studyId: 5,
        startFen: 'rnbqkbnr/pppppppp/8/8/4P3/8/PPPP1PPP/RNBQKBNR b KQkq - 0 1',
      },
    })
    expect(
      (black.find('[data-test="danger-side"]').element as HTMLSelectElement).value,
    ).toBe('Black')
  })

  it('never clobbers an in-progress spine edit when the start position changes', async () => {
    const wrapper = mount(DangerMapPanel, { props: { engineEnabled: true, studyId: 5 } })
    await wrapper.find('[data-test="danger-spine"]').setValue('1. e4 *')
    await wrapper.find('[data-test="danger-side"]').setValue('Black')

    await wrapper.setProps({
      startFen: 'rnbqkbnr/pppppppp/8/8/4P3/8/PPPP1PPP/RNBQKBNR b KQkq - 0 1',
    })

    expect(
      (wrapper.find('[data-test="danger-side"]').element as HTMLSelectElement).value,
    ).toBe('Black')
  })
})
