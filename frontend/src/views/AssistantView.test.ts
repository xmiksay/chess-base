// AssistantView's per-session model pickers (issue #215): the composer picker
// seeds newSession's choice, the header picker sends a mid-session set_model.
import { describe, it, expect, beforeEach, vi } from 'vitest'
import { flushPromises, mount, RouterLinkStub } from '@vue/test-utils'
import { setActivePinia, createPinia } from 'pinia'
import AssistantView from './AssistantView.vue'
import { useAssistantStore } from '../stores/assistant'
import { api } from '../api'
import type { ProviderInfo } from '../types'

vi.mock('../api', () => ({
  api: { providers: { list: vi.fn() } },
}))

// Same fake-socket double as stores/assistant.test.ts, trimmed to this test.
class FakeWebSocket {
  static last: FakeWebSocket
  readyState = 0
  sent: Array<Record<string, unknown>> = []
  onopen?: () => void
  onmessage?: (ev: { data: string }) => void
  onclose?: () => void
  onerror?: () => void
  constructor() {
    FakeWebSocket.last = this
  }
  send(data: string) {
    this.sent.push(JSON.parse(data))
  }
  close() {
    this.readyState = 3
  }
  open() {
    this.readyState = 1
    this.onopen?.()
  }
  emit(obj: unknown) {
    this.onmessage?.({ data: JSON.stringify(obj) })
  }
}

const SEP = '\u001f'
const ROOT = 'u1:abc'

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
    models: ['claude-a'],
    ...over,
  }
}

const rows = [row(), row({ id: 2, name: 'zai', model: 'glm-5', is_default: false, models: ['glm-5'] })]

function mountView() {
  const store = useAssistantStore()
  store._setSocketFactory(() => new FakeWebSocket() as unknown as WebSocket)
  const wrapper = mount(AssistantView, {
    global: { stubs: { RouterLink: RouterLinkStub, AssistantTranscript: true } },
  })
  FakeWebSocket.last.open()
  return { store, wrapper }
}

beforeEach(() => {
  setActivePinia(createPinia())
  vi.mocked(api.providers.list).mockResolvedValue(rows)
})

describe('AssistantView model pickers', () => {
  it('a new conversation submits the composer picker choice with the prompt', async () => {
    const { wrapper } = mountView()
    await flushPromises()

    const picker = wrapper.find('[data-test="new-model"] select')
    expect(picker.exists()).toBe(true)
    expect(wrapper.find('[data-test="session-model"]').exists()).toBe(false)

    await picker.setValue(`zai${SEP}glm-5`)
    await wrapper.find('textarea').setValue('build a repertoire')
    await wrapper.find('form.mt-3').trigger('submit')
    expect(FakeWebSocket.last.sent.at(-1)).toEqual({
      type: 'new',
      prompt: 'build a repertoire',
      name: null,
      provider: 'zai',
      model: 'glm-5',
    })
  })

  it('the header picker shows the transcript model and switches it mid-session', async () => {
    const { store, wrapper } = mountView()
    await flushPromises()
    store.currentRoot = ROOT
    FakeWebSocket.last.emit({
      type: 'out',
      ev: { kind: 'model_changed', session: ROOT, provider: 'anthropic', model: 'claude-a' },
    })
    await flushPromises()

    const picker = wrapper.find('[data-test="session-model"] select')
    expect(picker.exists()).toBe(true)
    expect((picker.element as HTMLSelectElement).value).toBe(`anthropic${SEP}claude-a`)
    expect(wrapper.find('[data-test="new-model"]').exists()).toBe(false)

    await picker.setValue(`zai${SEP}glm-5`)
    expect(FakeWebSocket.last.sent.at(-1)).toEqual({
      type: 'in',
      msg: { kind: 'set_model', session: ROOT, provider: 'zai', model: 'glm-5' },
    })
  })

  it('picking the default entry resolves it to the concrete default row', async () => {
    const { store, wrapper } = mountView()
    await flushPromises()
    store.currentRoot = ROOT
    FakeWebSocket.last.emit({
      type: 'out',
      ev: { kind: 'model_changed', session: ROOT, provider: 'zai', model: 'glm-5' },
    })
    await flushPromises()

    const picker = wrapper.find('[data-test="session-model"] select')
    await picker.setValue(`anthropic${SEP}claude-a`) // the "(default)" entry
    expect(FakeWebSocket.last.sent.at(-1)).toEqual({
      type: 'in',
      msg: { kind: 'set_model', session: ROOT, provider: 'anthropic', model: 'claude-a' },
    })
  })
})
