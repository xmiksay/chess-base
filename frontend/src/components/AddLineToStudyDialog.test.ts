import { describe, it, expect, beforeEach, vi } from 'vitest'
import { flushPromises, mount } from '@vue/test-utils'
import { setActivePinia, createPinia } from 'pinia'

// Mock the API client: the dialog drives studies.list/addLine, folders.list
// and databases.list.
vi.mock('../api', () => ({
  api: {
    studies: { list: vi.fn(), addLine: vi.fn() },
    folders: { list: vi.fn() },
    databases: { list: vi.fn() },
  },
}))

import { api } from '../api'
import AddLineToStudyDialog from './AddLineToStudyDialog.vue'
import type { Database, MoveStat, Study, StudySummary } from '../types'

const existingStudy: StudySummary = {
  id: 3,
  database_id: 1,
  name: 'Repertoire',
  global: false,
  owner_id: null,
  folder_id: null,
  origin_game_id: null,
}

const database: Database = {
  id: 1,
  owner_id: null,
  name: 'My games',
  kind: 'own',
  index_depth: null,
  global: false,
}

const addedStudy: Study = { ...existingStudy, tree: { root: 0, nodes: [] } }

async function setup(props: { sans: string[]; stat: MoveStat | null }) {
  const wrapper = mount(AddLineToStudyDialog, { props })
  await flushPromises()
  return wrapper
}

describe('AddLineToStudyDialog', () => {
  beforeEach(() => {
    setActivePinia(createPinia())
    vi.clearAllMocks()
    vi.mocked(api.studies.list).mockResolvedValue([existingStudy])
    vi.mocked(api.databases.list).mockResolvedValue([database])
    vi.mocked(api.folders.list).mockResolvedValue([])
    vi.mocked(api.studies.addLine).mockResolvedValue(addedStudy)
  })

  it('defaults to grafting into the first existing study', async () => {
    const wrapper = await setup({ sans: ['e4', 'e5'], stat: null })
    const select = wrapper.find('[data-test="study"]').element as HTMLSelectElement
    expect(select.value).toBe('3')
  })

  it('submits into the selected existing study', async () => {
    const wrapper = await setup({ sans: ['e4', 'e5'], stat: null })
    await wrapper.find('form').trigger('submit')
    await flushPromises()

    expect(api.studies.addLine).toHaveBeenCalledWith({
      sans: ['e4', 'e5'],
      study_id: 3,
      comment: undefined,
    })
    expect(wrapper.find('[data-test="add-line-result"]').text()).toContain('Repertoire')
  })

  it('switches to creating a new study and requires a name', async () => {
    const wrapper = await setup({ sans: ['e4'], stat: null })
    await wrapper.find('[data-test="mode-new"]').setValue()
    const submit = wrapper.find('[data-test="submit"]').element as HTMLButtonElement
    expect(submit.disabled).toBe(true)

    await wrapper.find('[data-test="name"]').setValue('New repertoire')
    expect(submit.disabled).toBe(false)

    await wrapper.find('form').trigger('submit')
    await flushPromises()
    expect(api.studies.addLine).toHaveBeenCalledWith({
      sans: ['e4'],
      database_id: 1,
      name: 'New repertoire',
      folder_id: null,
      comment: undefined,
    })
  })

  it('offers a stat comment, included by default', async () => {
    const stat: MoveStat = { san: 'e5', count: 4, white: 2, draws: 1, black: 1 }
    const wrapper = await setup({ sans: ['e4', 'e5'], stat })
    expect(wrapper.find('[data-test="include-comment"]').exists()).toBe(true)

    await wrapper.find('form').trigger('submit')
    await flushPromises()
    expect(api.studies.addLine).toHaveBeenCalledWith({
      sans: ['e4', 'e5'],
      study_id: 3,
      comment: '4 games, 2W/1D/1L',
    })
  })

  it('omits the comment when the checkbox is unchecked', async () => {
    const stat: MoveStat = { san: 'e5', count: 4, white: 2, draws: 1, black: 1 }
    const wrapper = await setup({ sans: ['e4', 'e5'], stat })
    await wrapper.find('[data-test="include-comment"]').setValue(false)

    await wrapper.find('form').trigger('submit')
    await flushPromises()
    expect(api.studies.addLine).toHaveBeenCalledWith({
      sans: ['e4', 'e5'],
      study_id: 3,
      comment: undefined,
    })
  })

  it('surfaces a submit failure', async () => {
    vi.mocked(api.studies.addLine).mockRejectedValue(
      new Error('cannot add a line to a study with a set-up start position'),
    )
    const wrapper = await setup({ sans: ['e4'], stat: null })
    await wrapper.find('form').trigger('submit')
    await flushPromises()
    expect(wrapper.find('[data-test="add-line-error"]').text()).toContain(
      'set-up start position',
    )
  })
})
