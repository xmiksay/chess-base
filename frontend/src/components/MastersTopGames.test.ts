import { describe, it, expect, beforeEach, vi } from 'vitest'
import { flushPromises, mount } from '@vue/test-utils'
import { setActivePinia, createPinia } from 'pinia'

vi.mock('../api', () => ({
  api: {
    whoami: vi.fn(),
    databases: { list: vi.fn() },
    explorer: { importMasters: vi.fn() },
  },
}))

import { api } from '../api'
import MastersTopGames from './MastersTopGames.vue'
import type { Database, MastersGame } from '../types'

const game: MastersGame = {
  id: 'abcd1234',
  uci: 'e2e4',
  winner: 'white',
  white: 'Kasparov, G.',
  white_rating: 2800,
  black: 'Karpov, A.',
  black_rating: 2750,
  year: 1990,
  month: '1990-10',
}

const dbs = [
  { id: 1, name: 'Global ref', global: true },
  { id: 2, name: 'Mine', global: false },
] as Database[]

async function setup() {
  const wrapper = mount(MastersTopGames, { props: { games: [game] } })
  await flushPromises()
  return wrapper
}

describe('MastersTopGames (ADR-0053)', () => {
  beforeEach(() => {
    setActivePinia(createPinia())
    vi.clearAllMocks()
    vi.mocked(api.whoami).mockResolvedValue({ id: 'u', is_admin: false })
    vi.mocked(api.databases.list).mockResolvedValue(dbs)
  })

  it('renders the game and offers only writable databases', async () => {
    const wrapper = await setup()
    const row = wrapper.find('[data-test="masters-game-row"]')
    expect(row.text()).toContain('Kasparov, G. (2800)')
    expect(row.text()).toContain('1-0')
    const options = wrapper.findAll('[data-test="masters-import-target"] option')
    expect(options.map((o) => o.text())).toEqual(['Mine'])
  })

  it('imports into the picked database and reports duplicates', async () => {
    vi.mocked(api.explorer.importMasters)
      .mockResolvedValueOnce({ imported: 1, duplicates: 0 })
      .mockResolvedValueOnce({ imported: 0, duplicates: 1 })
    const wrapper = await setup()

    await wrapper.find('[data-test="masters-import"]').trigger('click')
    await flushPromises()
    expect(api.explorer.importMasters).toHaveBeenCalledWith('abcd1234', 2)
    expect(wrapper.find('[data-test="masters-import-status"]').text()).toBe('Imported')

    await wrapper.find('[data-test="masters-import"]').trigger('click')
    await flushPromises()
    expect(wrapper.find('[data-test="masters-import-status"]').text()).toBe('Already in database')
  })

  it('disables import when nothing is writable', async () => {
    vi.mocked(api.databases.list).mockResolvedValue([dbs[0]])
    const wrapper = await setup()
    const button = wrapper.find('[data-test="masters-import"]').element as HTMLButtonElement
    expect(button.disabled).toBe(true)
  })
})
