import { describe, it, expect, vi, beforeEach } from 'vitest'
import { flushPromises, mount } from '@vue/test-utils'
import { setActivePinia, createPinia } from 'pinia'
import StudyView from './StudyView.vue'
import Board from '../components/Board.vue'
import { api } from '../api'
import { useStudiesStore } from '../stores/studies'
import { useStudyEditorStore } from '../stores/studyEditor'
import { useSettingsStore } from '../stores/settings'
import { useEngineStore } from '../stores/engine'
import { useDangerStore } from '../stores/danger'
import type { DangerTree, Study } from '../types'

vi.mock('../api', () => ({
  api: {
    health: vi.fn(),
    databases: { list: vi.fn() },
    studies: { list: vi.fn() },
    folders: { list: vi.fn() },
  },
}))

// One mainline move with a pinned plan arrow on the selected node (#61).
function study(): Study {
  return {
    id: 1,
    database_id: 1,
    name: 'Test study',
    global: false,
    owner_id: 'u1',
    folder_id: null,
    origin_game_id: null,
    tree: {
      root: 0,
      nodes: [
        { id: 0, parent: null, san: null, comment: null, nags: [], children: [1] },
        {
          id: 1,
          parent: 0,
          san: 'e4',
          comment: null,
          nags: [],
          children: [],
          shapes: [{ orig: 'd2', dest: 'd4', brush: 'green' }],
        },
      ],
    },
  }
}

beforeEach(() => {
  window.localStorage.clear()
  setActivePinia(createPinia())
  vi.clearAllMocks()
  vi.mocked(api.health).mockResolvedValue({ ok: true, llm: false } as never)
  vi.mocked(api.databases.list).mockResolvedValue([])
  vi.mocked(api.studies.list).mockResolvedValue([])
  vi.mocked(api.folders.list).mockResolvedValue([])
})

async function mountWithStudy() {
  // Keep the embedded engine panel (via StudyAnalysis) off the network.
  const engine = useEngineStore()
  vi.spyOn(engine, 'connect').mockImplementation(() => {})
  vi.spyOn(engine, 'disconnect').mockImplementation(() => {})

  const wrapper = mount(StudyView, { global: { stubs: { Board: true, RouterLink: true } } })
  await flushPromises()

  // Open a study by populating the stores directly (no network round-trip).
  const studies = useStudiesStore()
  const editor = useStudyEditorStore()
  studies.current = study()
  editor.select(1)
  await flushPromises()
  return wrapper
}

describe('StudyView', () => {
  it('renders the shared overlay toggles once a study is open', async () => {
    const wrapper = await mountWithStudy()
    expect(wrapper.find('[data-test="toggle-plans"]').exists()).toBe(true)
    expect(wrapper.find('[data-test="toggle-threats"]').exists()).toBe(true)
    expect(wrapper.find('[data-test="toggle-master"]').exists()).toBe(true)
  })

  it('composes the overlay layers with the node pinned shapes', async () => {
    const wrapper = await mountWithStudy()
    const board = wrapper.findComponent(Board)
    // The node's pinned drawing stays the editable `shapes` layer…
    expect(board.props('shapes')).toEqual([{ orig: 'd2', dest: 'd4', brush: 'green' }])
    // …while the toggleable overlays ride the read-only `overlayShapes` layer.
    // Plans default on, but the engine has no lines, so the union is empty.
    expect(board.props('overlayShapes')).toEqual([])
  })

  it('persists an overlay toggle through settings on change', async () => {
    const settings = useSettingsStore()
    const update = vi.spyOn(settings, 'update').mockResolvedValue()
    const wrapper = await mountWithStudy()

    await wrapper.find('[data-test="toggle-threats"]').setValue(true)
    expect(update).toHaveBeenCalledWith({ showThreats: true })
  })

  // Issue #190: "Clear arrows" must clear the live engine-analysis overlays
  // (including the danger layer) and keep them off, without ever touching the
  // node's persisted pinned shapes — those have their own "Clear pinned plan".
  it('clears the live overlay layers and leaves the pinned shape untouched', async () => {
    const settings = useSettingsStore()
    const update = vi.spyOn(settings, 'update').mockResolvedValue()
    const wrapper = await mountWithStudy()

    const editor = useStudyEditorStore()
    const danger = useDangerStore()
    // A danger node whose parent sits exactly at the selected node's FEN, so
    // `dangerShapesForFen` resolves an arrow for it (mirrors lib/dangerShapes.test.ts).
    danger.tree = {
      root: 0,
      nodes: [
        { id: 0, parent: null, ply: 0, children: [1], fen: editor.fen },
        {
          id: 1,
          parent: 0,
          ply: 1,
          san: 'e5',
          fen: 'irrelevant',
          children: [],
          tag: { kind: 'Trap', role: 'Caution', only_move_gap: 100, miss_rate: 0.3, eval: { cp: -20 } },
        },
      ],
    } as unknown as DangerTree
    await flushPromises()

    const board = wrapper.findComponent(Board)
    expect(board.props('overlayShapes')).toEqual([{ orig: 'e7', dest: 'e5', brush: 'dangerCaution' }])

    await wrapper.find('[data-test="clear-arrows"]').trigger('click')
    await flushPromises()

    expect(update).toHaveBeenCalledWith({ showPlans: false, showThreats: false, showMasterMoves: false })
    expect(board.props('overlayShapes')).toEqual([])
    // The node's persisted pinned shape (the editable `shapes` layer) is untouched.
    expect(board.props('shapes')).toEqual([{ orig: 'd2', dest: 'd4', brush: 'green' }])
  })
})
