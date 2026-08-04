import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest'
import { setActivePinia, createPinia } from 'pinia'
import { useAssistantStore } from './assistant'
import type { AssistantBubble } from '../lib/assistantStream'

// Minimal WebSocket double (the engine store's pattern): records sent frames
// and lets tests drive the open/message/close lifecycle by hand.
class FakeWebSocket {
  static last: FakeWebSocket
  static count = 0
  readyState = 0 // CONNECTING
  sent: Array<Record<string, unknown>> = []
  onopen?: () => void
  onmessage?: (ev: { data: string }) => void
  onclose?: () => void
  onerror?: () => void
  constructor() {
    FakeWebSocket.last = this
    FakeWebSocket.count += 1
  }
  send(data: string) {
    this.sent.push(JSON.parse(data))
  }
  close() {
    this.readyState = 3
    this.onclose?.()
  }
  open() {
    this.readyState = 1
    this.onopen?.()
  }
  emit(obj: unknown) {
    this.onmessage?.({ data: JSON.stringify(obj) })
  }
}

const ROOT = 'u1:abc'

function freshStore() {
  const store = useAssistantStore()
  store._setSocketFactory(() => new FakeWebSocket() as unknown as WebSocket)
  return store
}

/** A connected store with an open conversation (seeded by a first prompt). */
function openStore() {
  const store = freshStore()
  store.connect()
  FakeWebSocket.last.open()
  store.newSession('hi')
  FakeWebSocket.last.emit({ type: 'created', root: ROOT })
  FakeWebSocket.last.sent = []
  return store
}

function out(ev: Record<string, unknown>) {
  FakeWebSocket.last.emit({ type: 'out', ev })
}

describe('assistant store', () => {
  beforeEach(() => {
    setActivePinia(createPinia())
    FakeWebSocket.count = 0
  })
  afterEach(() => {
    vi.useRealTimers()
  })

  it('connects, lists on open, and answers ping with pong', () => {
    const store = freshStore()
    store.connect()
    FakeWebSocket.last.open()
    expect(store.connected).toBe(true)
    expect(FakeWebSocket.last.sent).toEqual([{ type: 'list' }])
    FakeWebSocket.last.emit({ type: 'ping' })
    expect(FakeWebSocket.last.sent.at(-1)).toEqual({ type: 'pong' })
  })

  it('newSession sends new, and created seeds the transcript with the prompt', () => {
    const store = freshStore()
    store.connect()
    FakeWebSocket.last.open()
    store.newSession('build a repertoire', null)
    expect(FakeWebSocket.last.sent.at(-1)).toEqual({
      type: 'new',
      prompt: 'build a repertoire',
      name: null,
    })
    FakeWebSocket.last.emit({ type: 'created', root: ROOT })
    expect(store.currentRoot).toBe(ROOT)
    expect(store.transcript.items).toEqual([{ type: 'user', text: 'build a repertoire' }])
    // The list refreshes so the sidebar picks the new conversation up.
    expect(FakeWebSocket.last.sent.at(-1)).toEqual({ type: 'list' })
  })

  it('newSession with a model choice adds provider/model to the new frame', () => {
    const store = freshStore()
    store.connect()
    FakeWebSocket.last.open()
    store.newSession('hi', null, { provider: 'zai', model: 'glm-5' })
    expect(FakeWebSocket.last.sent.at(-1)).toEqual({
      type: 'new',
      prompt: 'hi',
      name: null,
      provider: 'zai',
      model: 'glm-5',
    })
  })

  it('setModel ships a set_model InMsg for the open conversation', () => {
    const store = openStore()
    store.setModel('zai', 'glm-5')
    expect(FakeWebSocket.last.sent.at(-1)).toEqual({
      type: 'in',
      msg: { kind: 'set_model', session: ROOT, provider: 'zai', model: 'glm-5' },
    })
    // The engine confirms; the transcript's model tracks it.
    out({ kind: 'model_changed', session: ROOT, provider: 'zai', model: 'glm-5' })
    expect(store.transcript.model).toEqual({ provider: 'zai', model: 'glm-5' })

    // No open conversation ⇒ nothing is sent.
    store.deselect()
    FakeWebSocket.last.sent = []
    store.setModel('zai', 'glm-5')
    expect(FakeWebSocket.last.sent).toEqual([])
  })

  it('folds out events for the current root and ignores other sessions', () => {
    const store = openStore()
    out({ kind: 'text_delta', session: ROOT, seq: 1, text: 'hello' })
    out({ kind: 'text_delta', session: 'u1:other', seq: 1, text: 'IGNORED' })
    expect(store.transcript.items).toHaveLength(2) // user bubble + assistant
    expect((store.transcript.items[1] as AssistantBubble).text).toBe('hello')
    out({ kind: 'done', session: ROOT, seq: 2 })
    expect((store.transcript.items[1] as AssistantBubble).streaming).toBe(false)
  })

  it('send appends a user bubble and ships a prompt InMsg', () => {
    const store = openStore()
    store.send('next question')
    expect(store.transcript.items.at(-1)).toEqual({ type: 'user', text: 'next question' })
    expect(FakeWebSocket.last.sent.at(-1)).toEqual({
      type: 'in',
      msg: {
        kind: 'prompt',
        session: ROOT,
        content: [{ type: 'text', text: 'next question' }],
      },
    })
  })

  it('approve/reject resolve the card and send the right InMsg (scope omitted for once)', () => {
    const store = openStore()
    out({ kind: 'tool_request', session: ROOT, seq: 1, request_id: 'r1', tool: 'study_create', input: '{}' })
    expect(store.pendingApprovals).toHaveLength(1)

    store.approve('r1')
    expect(store.pendingApprovals).toHaveLength(0)
    expect(FakeWebSocket.last.sent.at(-1)).toEqual({
      type: 'in',
      msg: { kind: 'approve', session: ROOT, request_id: 'r1' },
    })

    out({ kind: 'tool_request', session: ROOT, seq: 2, request_id: 'r2', tool: 'study_create', input: '{}' })
    store.approve('r2', 'always')
    expect(FakeWebSocket.last.sent.at(-1)).toEqual({
      type: 'in',
      msg: { kind: 'approve', session: ROOT, request_id: 'r2', scope: 'always' },
    })

    out({ kind: 'tool_request', session: ROOT, seq: 3, request_id: 'r3', tool: 'game_delete', input: '{}' })
    store.reject('r3')
    expect(store.pendingApprovals).toHaveLength(0)
    expect(FakeWebSocket.last.sent.at(-1)).toEqual({
      type: 'in',
      msg: { kind: 'reject', session: ROOT, request_id: 'r3', reason: null },
    })
  })

  it('answerQuestion resolves the card and sends per-question answers', () => {
    const store = openStore()
    out({
      kind: 'user_question',
      session: ROOT,
      seq: 1,
      request_id: 'q1',
      questions: [{ question: 'Opening?', options: [{ label: 'Najdorf' }] }],
    })
    expect(store.pendingQuestions).toHaveLength(1)
    store.answerQuestion('q1', [['Najdorf']])
    expect(store.pendingQuestions).toHaveLength(0)
    expect(FakeWebSocket.last.sent.at(-1)).toEqual({
      type: 'in',
      msg: { kind: 'answer_question', session: ROOT, request_id: 'q1', answers: [['Najdorf']] },
    })
  })

  it('open replays history through the fold; a stale root is ignored', () => {
    const store = freshStore()
    store.connect()
    FakeWebSocket.last.open()
    store.open(ROOT)
    expect(FakeWebSocket.last.sent.at(-1)).toEqual({ type: 'open', root: ROOT })
    FakeWebSocket.last.emit({
      type: 'history',
      root: 'u1:stale',
      gapped: false,
      records: [{ dir: 'in', payload: { kind: 'prompt', session: 'u1:stale', content: [{ type: 'text', text: 'x' }] } }],
    })
    expect(store.transcript.items).toHaveLength(0)
    FakeWebSocket.last.emit({
      type: 'history',
      root: ROOT,
      gapped: true,
      records: [
        { dir: 'in', payload: { kind: 'prompt', session: ROOT, content: [{ type: 'text', text: 'hi' }] } },
        { dir: 'out', payload: { kind: 'text_delta', session: ROOT, seq: 1, text: 'yo' } },
        { dir: 'out', payload: { kind: 'done', session: ROOT, seq: 2 } },
      ],
    })
    expect(store.transcript.items).toEqual([
      { type: 'user', text: 'hi' },
      { type: 'assistant', text: 'yo', reasoning: '', streaming: false },
    ])
    expect(store.gapped).toBe(true)
  })

  it('session meta / lifecycle events touch the sidebar rows', () => {
    const store = openStore()
    FakeWebSocket.last.emit({
      type: 'sessions',
      items: [
        { root_session_id: ROOT, name: null, agent: 'chat', created_at: 't', last_active: 't', live: true },
      ],
    })
    out({ kind: 'session_meta_changed', session: ROOT, name: 'Sicilian prep', action: null })
    expect(store.sessions[0].name).toBe('Sicilian prep')
    out({ kind: 'session_ended', session: ROOT, ts: 1 })
    expect(store.sessions[0].live).toBe(false)
  })

  it('rename, stop and remove send their frames', () => {
    const store = openStore()
    store.rename(ROOT, 'New name')
    expect(FakeWebSocket.last.sent.at(-1)).toEqual({
      type: 'in',
      msg: { kind: 'set_session_meta', session: ROOT, name: 'New name', if_unset: false },
    })
    store.stop()
    expect(FakeWebSocket.last.sent.at(-1)).toEqual({
      type: 'in',
      msg: { kind: 'stop', session: ROOT },
    })
    store.remove(ROOT)
    expect(FakeWebSocket.last.sent.at(-1)).toEqual({ type: 'delete', root: ROOT })
    // The server confirms; the row and open conversation clear.
    FakeWebSocket.last.emit({ type: 'deleted', root: ROOT })
    expect(store.currentRoot).toBeNull()
  })

  it('surfaces gap/throttled banners and the no_provider error as unavailable', () => {
    const store = openStore()
    FakeWebSocket.last.emit({ type: 'gap', dropped: 12 })
    expect(store.gapped).toBe(true)
    FakeWebSocket.last.emit({ type: 'throttled', active: true })
    expect(store.throttled).toBe(true)
    FakeWebSocket.last.emit({ type: 'error', code: 'no_provider', message: 'no LLM provider configured' })
    expect(store.unavailable).toBe(true)
    expect(store.error).toContain('provider')
  })

  it('reconnects with backoff after an unexpected close, not after disconnect()', () => {
    vi.useFakeTimers()
    const store = freshStore()
    store.connect()
    const first = FakeWebSocket.last
    first.open()
    first.close() // dropped by the server
    expect(store.connected).toBe(false)
    expect(FakeWebSocket.count).toBe(1)
    vi.advanceTimersByTime(1000)
    expect(FakeWebSocket.count).toBe(2)

    // A second consecutive failure marks the assistant unavailable…
    FakeWebSocket.last.close()
    expect(store.unavailable).toBe(true)
    vi.advanceTimersByTime(2000)
    expect(FakeWebSocket.count).toBe(3)

    // …and a successful open clears it and re-lists.
    FakeWebSocket.last.open()
    expect(store.unavailable).toBe(false)

    store.disconnect()
    vi.advanceTimersByTime(60000)
    expect(FakeWebSocket.count).toBe(3) // no reconnect after a deliberate close
  })

  it('reopens the current conversation on reconnect', () => {
    vi.useFakeTimers()
    openStore()
    FakeWebSocket.last.close()
    vi.advanceTimersByTime(1000)
    FakeWebSocket.last.open()
    expect(FakeWebSocket.last.sent).toEqual([{ type: 'list' }, { type: 'open', root: ROOT }])
  })
})
