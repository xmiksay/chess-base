import { describe, it, expect, beforeEach, vi } from 'vitest'
import { flushPromises, mount } from '@vue/test-utils'
import { setActivePinia, createPinia } from 'pinia'

// Mock the API client: the sidebar drives the folders + studies stores.
vi.mock('../api', () => ({
  api: {
    folders: { list: vi.fn(), create: vi.fn() },
    studies: { list: vi.fn(), create: vi.fn(), setFolder: vi.fn() },
  },
}))

import { api } from '../api'
import StudyFolderSidebar from './StudyFolderSidebar.vue'
import { useFoldersStore } from '../stores/folders'
import { useStudiesStore } from '../stores/studies'
import type { FolderSummary, StudySummary } from '../types'

const folder = (id: number, parent_id: number | null = null, name = `F${id}`): FolderSummary => ({
  id,
  owner_id: null,
  parent_id,
  name,
  global: false,
})

const study = (id: number, folder_id: number | null): StudySummary => ({
  id,
  database_id: 1,
  name: `S${id}`,
  global: false,
  owner_id: null,
  folder_id,
  origin_game_id: null,
})

async function mountSidebar() {
  const folders = useFoldersStore()
  const studies = useStudiesStore()
  // Nested folders: 1 (root) → 2 (child); plus 3 (root).
  folders.list = [folder(1, null, 'Openings'), folder(2, 1, 'Sicilian'), folder(3, null, 'Endgames')]
  // Studies across the buckets: 10 unfiled, 11 in folder 1, 12 in folder 2.
  studies.list = [study(10, null), study(11, 1), study(12, 2)]
  const wrapper = mount(StudyFolderSidebar, {
    props: { databases: [], currentId: null, defaultDbId: null },
  })
  await flushPromises()
  return { wrapper, folders, studies }
}

describe('StudyFolderSidebar', () => {
  beforeEach(() => {
    setActivePinia(createPinia())
    vi.clearAllMocks()
    vi.mocked(api.folders.list).mockResolvedValue([])
    vi.mocked(api.studies.list).mockResolvedValue([])
  })

  it('renders the folder tree (root folders + nested children)', async () => {
    const { wrapper } = await mountSidebar()
    const rows = wrapper.findAll('[data-test="folder-row"]').map((r) => r.text())
    expect(rows).toContain('Openings')
    expect(rows).toContain('Sicilian') // the nested child renders too
    expect(rows).toContain('Endgames')
  })

  it('defaults to root and shows only the unfiled studies', async () => {
    const { wrapper } = await mountSidebar()
    const rows = wrapper.findAll('[data-test="study-row"]').map((r) => r.text())
    expect(rows).toEqual(['S10'])
  })

  it('selecting a folder filters the studies to that folder', async () => {
    const { wrapper } = await mountSidebar()
    // Click the "Openings" folder (id 1).
    const openings = wrapper
      .findAll('[data-test="folder-row"]')
      .find((r) => r.text() === 'Openings')!
    await openings.trigger('click')
    await flushPromises()
    const rows = wrapper.findAll('[data-test="study-row"]').map((r) => r.text())
    expect(rows).toEqual(['S11'])
  })

  it('moving a study calls setFolder and refreshes', async () => {
    const { wrapper, studies } = await mountSidebar()
    const setFolder = vi.spyOn(studies, 'setFolder').mockResolvedValue({} as never)
    // The visible (root) study is S10; move it to folder 2.
    const select = wrapper.find('[data-test="move-study"]')
    await select.setValue('2')
    expect(setFolder).toHaveBeenCalledWith(10, 2)
  })
})
