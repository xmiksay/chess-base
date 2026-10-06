import { describe, it, expect, vi, beforeEach } from 'vitest'
import { flushPromises, mount } from '@vue/test-utils'
import UsersSettings from './UsersSettings.vue'
import { api } from '../api'
import type { UserSummary } from '../types'

vi.mock('../api', () => ({
  api: { users: { list: vi.fn(), resetPassword: vi.fn() } },
}))

const USERS: UserSummary[] = [
  { id: 'a1', username: 'alice', is_admin: true, created_at: '2026-01-02T10:00:00' },
  { id: 'b2', username: 'bob', is_admin: false, created_at: '2026-03-04T10:00:00' },
]

beforeEach(() => {
  vi.clearAllMocks()
  vi.mocked(api.users.list).mockResolvedValue(USERS)
})

describe('UsersSettings', () => {
  it('lists users and offers reset on everyone but yourself', async () => {
    const wrapper = mount(UsersSettings, { props: { selfId: 'a1' } })
    await flushPromises()
    const rows = wrapper.findAll('[data-test="user-row"]')
    expect(rows).toHaveLength(2)
    expect(rows[0].text()).toContain('admin')
    expect(rows[0].find('[data-test="user-reset"]').exists()).toBe(false)
    expect(rows[1].find('[data-test="user-reset"]').exists()).toBe(true)
  })

  it('resets a password after matching confirmation', async () => {
    vi.mocked(api.users.resetPassword).mockResolvedValue(null)
    const wrapper = mount(UsersSettings, { props: { selfId: 'a1' } })
    await flushPromises()
    await wrapper.find('[data-test="user-reset"]').trigger('click')
    await wrapper.find('[data-test="reset-new"]').setValue('fresh-pass1')
    await wrapper.find('[data-test="reset-confirm"]').setValue('mismatch1')
    await wrapper.find('[data-test="reset-form"]').trigger('submit')
    await flushPromises()
    expect(api.users.resetPassword).not.toHaveBeenCalled()
    expect(wrapper.find('[data-test="users-error"]').exists()).toBe(true)

    await wrapper.find('[data-test="reset-confirm"]').setValue('fresh-pass1')
    await wrapper.find('[data-test="reset-form"]').trigger('submit')
    await flushPromises()
    expect(api.users.resetPassword).toHaveBeenCalledWith('b2', 'fresh-pass1')
    expect(wrapper.find('[data-test="users-notice"]').text()).toContain('bob')
    expect(wrapper.find('[data-test="reset-form"]').exists()).toBe(false)
  })
})
