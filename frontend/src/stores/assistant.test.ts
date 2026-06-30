import { describe, it, expect, beforeEach, vi } from 'vitest'
import { setActivePinia, createPinia } from 'pinia'

// Mock the API client so the store is tested against fakes, no network.
vi.mock('../api', () => ({
  api: {
    assistant: {
      listSessions: vi.fn(),
      getSession: vi.fn(),
      createSession: vi.fn(),
      deleteSession: vi.fn(),
      send: vi.fn(),
      respond: vi.fn(),
    },
  },
}))

import { api } from '../api'
import { useAssistantStore } from './assistant'
import type { AssistantSession } from '../types'

function session(over: Partial<AssistantSession> = {}): AssistantSession {
  return {
    id: 1,
    title: 'New chat',
    model: 'claude-sonnet-4-6',
    messages: [],
    pending_approvals: [],
    awaiting_approval: false,
    iterations: 0,
    iteration_cap: 8,
    ...over,
  }
}

describe('assistant store', () => {
  beforeEach(() => {
    setActivePinia(createPinia())
    vi.clearAllMocks()
    vi.mocked(api.assistant.listSessions).mockResolvedValue([])
  })

  it('create opens the new session and refreshes the list', async () => {
    vi.mocked(api.assistant.createSession).mockResolvedValue(session({ id: 5 }))
    const store = useAssistantStore()
    await store.create()
    expect(store.current?.id).toBe(5)
    expect(api.assistant.listSessions).toHaveBeenCalled()
  })

  it('exposes pending approvals when the loop pauses', async () => {
    const paused = session({
      awaiting_approval: true,
      pending_approvals: [
        { id: 'c1', name: 'study_create', input: {}, requires_approval: true },
      ],
    })
    vi.mocked(api.assistant.createSession).mockResolvedValue(session())
    vi.mocked(api.assistant.send).mockResolvedValue(paused)
    const store = useAssistantStore()
    await store.create()
    await store.sendMessage('build a Sicilian repertoire')
    expect(store.awaitingApproval).toBe(true)
    expect(store.pending).toHaveLength(1)
    expect(store.pending[0].name).toBe('study_create')
  })

  it('resolveAll sends an approve/deny decision for every pending call', async () => {
    vi.mocked(api.assistant.createSession).mockResolvedValue(
      session({
        awaiting_approval: true,
        pending_approvals: [
          { id: 'c1', name: 'study_create', input: {}, requires_approval: true },
          { id: 'c2', name: 'study_add_move', input: {}, requires_approval: true },
        ],
      }),
    )
    vi.mocked(api.assistant.respond).mockResolvedValue(session())
    const store = useAssistantStore()
    await store.create()
    await store.resolveAll(true)
    expect(api.assistant.respond).toHaveBeenCalledWith(1, { c1: true, c2: true })
    expect(store.awaitingApproval).toBe(false)
  })
})
