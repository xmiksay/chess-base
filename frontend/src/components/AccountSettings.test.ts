import { describe, it, expect, vi, beforeEach } from 'vitest'
import { flushPromises, mount } from '@vue/test-utils'
import AccountSettings from './AccountSettings.vue'
import { api } from '../api'

vi.mock('../api', () => ({
  api: { auth: { changePassword: vi.fn() } },
}))

async function fill(wrapper: ReturnType<typeof mount>, current: string, next: string, confirm: string) {
  await wrapper.find('[data-test="password-current"]').setValue(current)
  await wrapper.find('[data-test="password-new"]').setValue(next)
  await wrapper.find('[data-test="password-confirm"]').setValue(confirm)
  await wrapper.find('[data-test="password-form"]').trigger('submit')
  await flushPromises()
}

beforeEach(() => vi.clearAllMocks())

describe('AccountSettings', () => {
  it('changes the password and clears the form', async () => {
    vi.mocked(api.auth.changePassword).mockResolvedValue(null)
    const wrapper = mount(AccountSettings)
    await fill(wrapper, 'password123', 'newpass456', 'newpass456')
    expect(api.auth.changePassword).toHaveBeenCalledWith('password123', 'newpass456')
    expect(wrapper.find('[data-test="password-done"]').exists()).toBe(true)
    expect((wrapper.find('[data-test="password-current"]').element as HTMLInputElement).value).toBe('')
  })

  it('refuses a mismatched confirmation without calling the API', async () => {
    const wrapper = mount(AccountSettings)
    await fill(wrapper, 'password123', 'newpass456', 'different1')
    expect(api.auth.changePassword).not.toHaveBeenCalled()
    expect(wrapper.find('[data-test="password-error"]').text()).toContain('do not match')
  })

  it('shows the server error', async () => {
    vi.mocked(api.auth.changePassword).mockRejectedValue(new Error('current password is incorrect'))
    const wrapper = mount(AccountSettings)
    await fill(wrapper, 'wrong-one', 'newpass456', 'newpass456')
    expect(wrapper.find('[data-test="password-error"]').text()).toContain('incorrect')
    expect(wrapper.find('[data-test="password-done"]').exists()).toBe(false)
  })
})
