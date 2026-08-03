import { describe, it, expect, beforeEach, vi } from 'vitest'
import { mount, flushPromises } from '@vue/test-utils'
import ModelSelect from './ModelSelect.vue'
import { api } from '../api'
import type { ProviderInfo } from '../types'

vi.mock('../api', () => ({
  api: { providers: { list: vi.fn() } },
}))

const SEP = '\u001f'

function row(over: Partial<ProviderInfo> = {}): ProviderInfo {
  return {
    id: 1,
    name: 'anthropic',
    wire: 'anthropic',
    model: 'claude-a',
    base_url: null,
    has_key: true,
    is_default: true,
    is_global: false,
    models: ['claude-a', 'claude-b'],
    ...over,
  }
}

const rows = [
  row(),
  row({ id: 2, name: 'zai', model: 'glm-5', is_default: false, models: ['glm-5'] }),
]

beforeEach(() => {
  vi.clearAllMocks()
})

describe('ModelSelect', () => {
  it('renders one optgroup per provider with its models, default labeled and preselected', () => {
    const wrapper = mount(ModelSelect, {
      props: { modelValue: null, providers: rows },
    })
    const groups = wrapper.findAll('optgroup')
    expect(groups.map((g) => g.attributes('label'))).toEqual(['anthropic', 'zai'])
    const options = wrapper.findAll('option')
    expect(options).toHaveLength(3)
    expect(options[0].text()).toBe('claude-a (default)')
    expect(options[1].text()).toBe('claude-b')
    const select = wrapper.find('select').element as HTMLSelectElement
    expect(select.value).toBe(`anthropic${SEP}claude-a`)
  })

  it('emits the picked provider/model, and null when back on the default', async () => {
    const wrapper = mount(ModelSelect, {
      props: { modelValue: null, providers: rows },
    })
    await wrapper.find('select').setValue(`zai${SEP}glm-5`)
    expect(wrapper.emitted('update:modelValue')![0]).toEqual([
      { provider: 'zai', model: 'glm-5' },
    ])

    // Re-selecting the effective default means "use default" ⇒ null.
    await wrapper.setProps({ modelValue: { provider: 'zai', model: 'glm-5' } })
    await wrapper.find('select').setValue(`anthropic${SEP}claude-a`)
    expect(wrapper.emitted('update:modelValue')![1]).toEqual([null])
  })

  it('fetches the provider list itself when no rows are passed', async () => {
    vi.mocked(api.providers.list).mockResolvedValue(rows)
    const wrapper = mount(ModelSelect, { props: { modelValue: null } })
    await flushPromises()
    expect(api.providers.list).toHaveBeenCalledOnce()
    expect(wrapper.findAll('optgroup')).toHaveLength(2)
  })

  it('shows a disabled placeholder when nothing is configured', async () => {
    vi.mocked(api.providers.list).mockRejectedValue(new Error('offline'))
    const wrapper = mount(ModelSelect, { props: { modelValue: null } })
    await flushPromises()
    const option = wrapper.find('option')
    expect(option.text()).toContain('No LLM providers configured')
    expect(option.attributes('disabled')).toBeDefined()
  })
})
