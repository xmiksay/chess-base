import { describe, it, expect, beforeEach, vi } from 'vitest'
import { flushPromises, mount } from '@vue/test-utils'
import { setActivePinia, createPinia } from 'pinia'

// Drives api.studies.mergeGames + the studies/folders lists for the pickers.
vi.mock('../api', () => ({
  api: {
    studies: { list: vi.fn(), mergeGames: vi.fn() },
    folders: { list: vi.fn() },
  },
}))

import { api } from '../api'
import MergeGamesDialog from './MergeGamesDialog.vue'
import type { Study, StudySummary } from '../types'

const existingStudy: StudySummary = {
  id: 4,
  database_id: 1,
  name: 'Najdorf repertoire',
  global: false,
  owner_id: 'alice',
  folder_id: null,
  origin_game_id: null,
}

async function setup(gameIds = [1, 2]) {
  const wrapper = mount(MergeGamesDialog, { props: { gameIds } })
  await flushPromises()
  return wrapper
}

describe('MergeGamesDialog', () => {
  beforeEach(() => {
    setActivePinia(createPinia())
    vi.clearAllMocks()
    vi.mocked(api.studies.list).mockResolvedValue([existingStudy])
    vi.mocked(api.folders.list).mockResolvedValue([])
  })

  it('disables submit until a new-study name is entered', async () => {
    const wrapper = await setup()
    const submit = wrapper.find('[data-test="merge-submit"]')
    expect((submit.element as HTMLButtonElement).disabled).toBe(true)

    await wrapper.find('[data-test="merge-new-name"]').setValue('Repertoire')
    expect((submit.element as HTMLButtonElement).disabled).toBe(false)
  })

  it('creates a new study by default (study_id omitted)', async () => {
    const created = { id: 9, name: 'Repertoire' } as Study
    vi.mocked(api.studies.mergeGames).mockResolvedValue(created)

    const wrapper = await setup([1, 2, 3])
    await wrapper.find('[data-test="merge-new-name"]').setValue('Repertoire')
    await wrapper.find('form').trigger('submit')
    await flushPromises()

    expect(api.studies.mergeGames).toHaveBeenCalledWith({
      game_ids: [1, 2, 3],
      name: 'Repertoire',
      folder_id: null,
    })
    expect(wrapper.emitted('merged')?.[0]).toEqual([created])
  })

  it('merges into an existing study when one is picked (name not required)', async () => {
    const merged = { id: 4, name: 'Najdorf repertoire' } as Study
    vi.mocked(api.studies.mergeGames).mockResolvedValue(merged)

    const wrapper = await setup()
    await wrapper.find('[data-test="merge-target-study"]').setValue('4')
    expect((wrapper.find('[data-test="merge-submit"]').element as HTMLButtonElement).disabled).toBe(
      false,
    )
    await wrapper.find('form').trigger('submit')
    await flushPromises()

    expect(api.studies.mergeGames).toHaveBeenCalledWith({
      game_ids: [1, 2],
      study_id: 4,
    })
    expect(wrapper.emitted('merged')?.[0]).toEqual([merged])
  })

  it('surfaces a merge failure', async () => {
    vi.mocked(api.studies.mergeGames).mockRejectedValue(new Error('no mergeable games'))

    const wrapper = await setup()
    await wrapper.find('[data-test="merge-new-name"]').setValue('Repertoire')
    await wrapper.find('form').trigger('submit')
    await flushPromises()

    expect(wrapper.find('[data-test="merge-error"]').text()).toContain('no mergeable games')
  })

  it('emits close on cancel', async () => {
    const wrapper = await setup()
    await wrapper.find('button[type="button"]').trigger('click')
    expect(wrapper.emitted('close')).toBeTruthy()
  })
})
