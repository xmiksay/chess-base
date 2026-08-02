import { describe, it, expect, vi, beforeEach } from 'vitest'
import { flushPromises, mount } from '@vue/test-utils'
import ProvidersSettings from './ProvidersSettings.vue'
import { api } from '../api'
import type { ProviderInfo } from '../types'

vi.mock('../api', () => ({
  api: {
    providers: { list: vi.fn(), upsert: vi.fn(), remove: vi.fn() },
    whoami: vi.fn(),
  },
}))

function provider(over: Partial<ProviderInfo> = {}): ProviderInfo {
  return {
    id: 1,
    name: 'My Anthropic',
    wire: 'anthropic',
    model: 'claude-sonnet-4-6',
    base_url: null,
    has_key: true,
    is_default: true,
    is_global: false,
    ...over,
  }
}

beforeEach(() => {
  vi.clearAllMocks()
  vi.mocked(api.providers.list).mockResolvedValue([provider()])
  vi.mocked(api.whoami).mockResolvedValue({ id: 'u1', is_admin: false })
})

describe('ProvidersSettings', () => {
  it('lists providers with wire/model, key badge and default star', async () => {
    const wrapper = mount(ProvidersSettings)
    await flushPromises()
    const row = wrapper.find('[data-test="provider-row"]')
    expect(row.text()).toContain('My Anthropic')
    expect(row.text()).toContain('anthropic · claude-sonnet-4-6')
    expect(row.text()).toContain('key set')
    expect(row.text()).toContain('★')
  })

  it('submits a new provider and omits a blank api_key on edit', async () => {
    vi.mocked(api.providers.upsert).mockResolvedValue(provider())
    const wrapper = mount(ProvidersSettings)
    await flushPromises()

    // Edit the existing row without touching the key: api_key must be omitted.
    await wrapper.find('[data-test="provider-row"] button').trigger('click') // edit
    await wrapper.find('[data-test="provider-model"]').setValue('claude-opus-4-5')
    await wrapper.find('[data-test="provider-form"]').trigger('submit')
    await flushPromises()

    expect(api.providers.upsert).toHaveBeenCalledWith({
      name: 'My Anthropic',
      wire: 'anthropic',
      model: 'claude-opus-4-5',
      base_url: null,
      is_default: true,
      is_global: false,
    })
    const body = vi.mocked(api.providers.upsert).mock.calls[0][0]
    expect('api_key' in body).toBe(false)
  })

  it('sends the api_key when one is typed', async () => {
    vi.mocked(api.providers.upsert).mockResolvedValue(provider())
    const wrapper = mount(ProvidersSettings)
    await flushPromises()

    await wrapper.find('[data-test="provider-add"]').trigger('click')
    await wrapper.find('[data-test="provider-name"]').setValue('OpenAI')
    await wrapper.find('[data-test="provider-wire"]').setValue('openai')
    await wrapper.find('[data-test="provider-model"]').setValue('gpt-5')
    await wrapper.find('[data-test="provider-key"]').setValue('sk-secret')
    await wrapper.find('[data-test="provider-form"]').trigger('submit')
    await flushPromises()

    expect(api.providers.upsert).toHaveBeenCalledWith(
      expect.objectContaining({ name: 'OpenAI', wire: 'openai', api_key: 'sk-secret' }),
    )
  })

  it('hides the global toggle for non-admins and shows it for admins', async () => {
    const wrapper = mount(ProvidersSettings)
    await flushPromises()
    await wrapper.find('[data-test="provider-add"]').trigger('click')
    expect(wrapper.find('[data-test="provider-global"]').exists()).toBe(false)

    vi.mocked(api.whoami).mockResolvedValue({ id: 'admin', is_admin: true })
    const adminWrapper = mount(ProvidersSettings)
    await flushPromises()
    await adminWrapper.find('[data-test="provider-add"]').trigger('click')
    expect(adminWrapper.find('[data-test="provider-global"]').exists()).toBe(true)
  })

  it('deletes after confirmation', async () => {
    vi.mocked(api.providers.remove).mockResolvedValue(null)
    vi.spyOn(window, 'confirm').mockReturnValue(true)
    const wrapper = mount(ProvidersSettings)
    await flushPromises()

    const buttons = wrapper.findAll('[data-test="provider-row"] button')
    await buttons[buttons.length - 1].trigger('click')
    await flushPromises()
    expect(api.providers.remove).toHaveBeenCalledWith(1)
  })
})
