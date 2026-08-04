import { describe, it, expect, vi, beforeEach } from 'vitest'
import { flushPromises, mount, RouterLinkStub } from '@vue/test-utils'
import { setActivePinia, createPinia } from 'pinia'
import StudyView from './StudyView.vue'
import Board from '../components/Board.vue'
import MoveTree from '../components/MoveTree.vue'
import AnnotationEditor from '../components/AnnotationEditor.vue'
import { api } from '../api'
import { useAuthStore } from '../stores/auth'
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

// Router mock (issue #212): a mutable route each test seeds before mount, plus
// push/replace spies for asserting the mirrored navigation. `resolve` backs
// the share-link URL (issue #213).
const { push, replace, resolve, route } = vi.hoisted(() => ({
  push: vi.fn(),
  replace: vi.fn(),
  resolve: vi.fn((to: { params?: { id?: string } }) => ({ href: `/studies/${to.params?.id ?? ''}` })),
  route: {
    name: 'studies' as string,
    params: {} as Record<string, string>,
    query: {} as Record<string, string>,
  },
}))
vi.mock('vue-router', () => ({
  useRouter: () => ({ push, replace, resolve }),
  useRoute: () => route,
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
    origin_game_id: null, public: false,
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
  route.name = 'studies'
  route.params = {}
  route.query = {}
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

  const wrapper = mount(StudyView, { global: { stubs: { Board: true, RouterLink: RouterLinkStub } } })
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

// Issue #212: /studies/:id? + ?folder= — hydrate from the URL, mirror selection.
describe('StudyView URL addressability', () => {
  it('opens the study named by /studies/:id on mount', async () => {
    route.params = { id: '45' }
    const editor = useStudyEditorStore()
    const open = vi.spyOn(editor, 'open').mockResolvedValue(undefined)

    mount(StudyView, { global: { stubs: { Board: true, RouterLink: true } } })
    await flushPromises()

    expect(open).toHaveBeenCalledWith(45)
  })

  it('mirrors the open study into the URL via router.replace', async () => {
    await mountWithStudy() // sets studies.current (id 1) after mount
    expect(replace).toHaveBeenCalledWith(
      expect.objectContaining({ name: 'studies', params: { id: '1' } }),
    )
  })

  it('links the origin game with its id in the route params', async () => {
    const wrapper = await mountWithStudy()
    const studies = useStudiesStore()
    studies.current = { ...study(), origin_game_id: 7, public: false }
    await flushPromises()

    const link = wrapper
      .findAllComponents(RouterLinkStub)
      .find((l) => l.attributes('data-test') === 'origin-game-link')!
    expect(link.props('to')).toEqual({ name: 'games', params: { id: '7' } })
  })
})

// Issue #213: a logged-out server-mode visitor on /studies/:id sees the study
// read-only — no sidebar, no engine analysis, no editing controls.
describe('StudyView anonymous read-only view', () => {
  it('hydrates only the deep-linked study and hides the editing chrome', async () => {
    route.params = { id: '1' }
    const auth = useAuthStore()
    auth.mode = 'server'
    auth.user = null

    const studies = useStudiesStore()
    const editor = useStudyEditorStore()
    const open = vi.spyOn(editor, 'open').mockImplementation(async (id: number) => {
      studies.current = { ...study(), id, public: true }
    })

    const wrapper = mount(StudyView, {
      global: { stubs: { Board: true, RouterLink: RouterLinkStub } },
    })
    await flushPromises()

    expect(open).toHaveBeenCalledWith(1)
    // The authenticated refreshes are skipped — they would all 401.
    expect(api.studies.list).not.toHaveBeenCalled()
    expect(api.folders.list).not.toHaveBeenCalled()
    expect(api.databases.list).not.toHaveBeenCalled()

    // No sidebar, generate buttons, engine analysis or annotation editing.
    expect(wrapper.findComponent({ name: 'StudyFolderSidebar' }).exists()).toBe(false)
    expect(wrapper.find('[data-test="open-generate"]').exists()).toBe(false)
    expect(wrapper.find('[data-test="open-danger-map"]').exists()).toBe(false)
    expect(wrapper.findComponent({ name: 'StudyAnalysis' }).exists()).toBe(false)
    expect(wrapper.findComponent({ name: 'DangerMapPanel' }).exists()).toBe(false)
    expect(wrapper.findComponent(AnnotationEditor).exists()).toBe(false)

    // The tree renders but is not editable; the board takes no moves/drawings.
    expect(wrapper.findComponent(MoveTree).props('editable')).toBe(false)
    const board = wrapper.findComponent(Board)
    expect(board.props('movable')).toBe(false)
    expect(board.props('editableShapes')).toBe(false)

    // PGN export + the share pill/copy-link stay; no share checkbox anonymously.
    expect(wrapper.find('[data-test="export"]').exists()).toBe(true)
    expect(wrapper.find('[data-test="export-eval"]').exists()).toBe(true)
    expect(wrapper.find('[data-test="share-pill"]').exists()).toBe(true)
    expect(wrapper.find('[data-test="copy-link"]').exists()).toBe(true)
    expect(wrapper.find('[data-test="share-toggle"]').exists()).toBe(false)
  })
})
