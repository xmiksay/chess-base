import { describe, it, expect } from 'vitest'
import {
  emptyTranscript,
  foldEvent,
  foldHistory,
  foldPrompt,
  parseServerFrame,
  prettyJson,
  resolveApproval,
  resolveQuestion,
} from './assistantStream'
import type { AssistantBubble, ApprovalCard, ToolChip, TranscriptState } from './assistantStream'
import type { AssistantHistoryRecord, AssistantOutEvent } from '../types'

const S = 'user:root'

function fold(events: AssistantOutEvent[], state: TranscriptState = emptyTranscript()) {
  return events.reduce(foldEvent, state)
}

describe('parseServerFrame', () => {
  it('parses type-tagged envelope frames', () => {
    expect(parseServerFrame('{"type":"ping"}')).toEqual({ type: 'ping' })
    expect(parseServerFrame('{"type":"gap","dropped":7}')).toEqual({ type: 'gap', dropped: 7 })
    expect(parseServerFrame('{"type":"throttled","active":true}')).toEqual({
      type: 'throttled',
      active: true,
    })
  })

  it('rejects non-JSON and untyped payloads', () => {
    expect(parseServerFrame('nonsense')).toBeNull()
    expect(parseServerFrame('42')).toBeNull()
    expect(parseServerFrame('{"kind":"done"}')).toBeNull()
  })
})

describe('foldEvent — deltas', () => {
  it('accumulates text deltas into one streaming bubble', () => {
    const state = fold([
      { kind: 'text_delta', session: S, seq: 1, text: 'Hel' },
      { kind: 'text_delta', session: S, seq: 2, text: 'lo' },
    ])
    expect(state.items).toHaveLength(1)
    const bubble = state.items[0] as AssistantBubble
    expect(bubble.type).toBe('assistant')
    expect(bubble.text).toBe('Hello')
    expect(bubble.streaming).toBe(true)
  })

  it('folds reasoning deltas into the same open bubble', () => {
    const state = fold([
      { kind: 'reasoning_delta', session: S, seq: 1, text: 'thinking…' },
      { kind: 'text_delta', session: S, seq: 2, text: 'answer' },
    ])
    expect(state.items).toHaveLength(1)
    const bubble = state.items[0] as AssistantBubble
    expect(bubble.reasoning).toBe('thinking…')
    expect(bubble.text).toBe('answer')
  })

  it('done closes the bubble; later deltas open a fresh one', () => {
    const state = fold([
      { kind: 'text_delta', session: S, seq: 1, text: 'first' },
      { kind: 'done', session: S, seq: 2 },
      { kind: 'text_delta', session: S, seq: 3, text: 'second' },
    ])
    expect(state.items).toHaveLength(2)
    expect((state.items[0] as AssistantBubble).streaming).toBe(false)
    expect((state.items[1] as AssistantBubble).streaming).toBe(true)
    expect((state.items[1] as AssistantBubble).text).toBe('second')
  })

  it('does not mutate the previous state', () => {
    const before = fold([{ kind: 'text_delta', session: S, seq: 1, text: 'a' }])
    fold([{ kind: 'text_delta', session: S, seq: 2, text: 'b' }], before)
    expect((before.items[0] as AssistantBubble).text).toBe('a')
  })
})

describe('foldEvent — tool chips', () => {
  it('creates one chip per request across delta/call/exec and closes it on output', () => {
    const state = fold([
      { kind: 'tool_call_delta', session: S, seq: 1, request_id: 'r1', tool: 'study_get', delta: '{"' },
      { kind: 'tool_call', session: S, seq: 2, request_id: 'r1', tool: 'study_get', input: '{}' },
      { kind: 'tool_exec', session: S, seq: 3, request_id: 'r1', tool: 'study_get', input: '{}' },
    ])
    expect(state.items.filter((i) => i.type === 'tool')).toHaveLength(1)
    expect((state.items[0] as ToolChip).state).toBe('running')

    const after = foldEvent(state, {
      kind: 'tool_output',
      session: S,
      seq: 4,
      request_id: 'r1',
      tool: 'study_get',
      output: 'ok',
    })
    expect((after.items[0] as ToolChip).state).toBe('done')
  })

  it('a chip closes the open bubble so later text starts a new segment', () => {
    const state = fold([
      { kind: 'text_delta', session: S, seq: 1, text: 'let me check' },
      { kind: 'tool_call', session: S, seq: 2, request_id: 'r1', tool: 'db_read_game', input: '{}' },
      { kind: 'text_delta', session: S, seq: 3, text: 'found it' },
    ])
    expect(state.items.map((i) => i.type)).toEqual(['assistant', 'tool', 'assistant'])
    expect((state.items[0] as AssistantBubble).streaming).toBe(false)
  })
})

describe('foldEvent — approval cards', () => {
  const request: AssistantOutEvent = {
    kind: 'tool_request',
    session: S,
    seq: 1,
    request_id: 'r9',
    tool: 'study_create',
    input: '{"name":"Repertoire"}',
  }

  it('tool_request opens a card with pretty-printed input', () => {
    const state = fold([request])
    const card = state.items[0] as ApprovalCard
    expect(card.type).toBe('approval')
    expect(card.resolved).toBeNull()
    expect(card.prettyInput).toBe('{\n  "name": "Repertoire"\n}')
  })

  it('a matching tool_exec resolves the card approved', () => {
    const state = fold([
      request,
      { kind: 'tool_exec', session: S, seq: 2, request_id: 'r9', tool: 'study_create', input: '{}' },
    ])
    const card = state.items.find((i) => i.type === 'approval') as ApprovalCard
    expect(card.resolved).toBe('approved')
  })

  it('a tool_output without exec resolves the card rejected', () => {
    const state = fold([
      request,
      { kind: 'tool_output', session: S, seq: 2, request_id: 'r9', tool: 'study_create', output: 'denied' },
    ])
    const card = state.items.find((i) => i.type === 'approval') as ApprovalCard
    expect(card.resolved).toBe('rejected')
  })

  it('resolveApproval marks a pending card and is a no-op when resolved', () => {
    let state = fold([request])
    state = resolveApproval(state, 'r9', 'rejected')
    expect((state.items[0] as ApprovalCard).resolved).toBe('rejected')
    // A later exec must not flip a user rejection.
    const after = resolveApproval(state, 'r9', 'approved')
    expect((after.items[0] as ApprovalCard).resolved).toBe('rejected')
  })
})

describe('foldEvent — questions, status, usage, notices', () => {
  it('user_question opens a card and resolveQuestion marks it answered', () => {
    let state = fold([
      {
        kind: 'user_question',
        session: S,
        seq: 1,
        request_id: 'q1',
        questions: [{ question: 'Which opening?', options: [{ label: 'Najdorf' }] }],
      },
    ])
    expect(state.items[0]).toMatchObject({ type: 'question', resolved: false })
    state = resolveQuestion(state, 'q1')
    expect(state.items[0]).toMatchObject({ resolved: true })
  })

  it('tracks status and accumulates usage totals', () => {
    const usage = {
      kind: 'usage',
      session: S,
      seq: 1,
      input_tokens: 100,
      output_tokens: 20,
      cached_input_tokens: 0,
      cache_write_tokens: 0,
      cost_usd: 0.01,
    } as const
    const state = fold([
      { kind: 'status', session: S, state: 'thinking' },
      usage,
      { ...usage, seq: 2, input_tokens: 50, output_tokens: 5, cost_usd: null },
    ])
    expect(state.status).toBe('thinking')
    expect(state.usage).toEqual({ input_tokens: 150, output_tokens: 25, cost_usd: 0.01 })
  })

  it('errors surface as items, compaction as a divider, plan/task_list drop', () => {
    const state = fold([
      { kind: 'error', session: S, seq: 1, message: 'boom' },
      { kind: 'compacted', session: S, seq: 2, summary: 's' },
      { kind: 'plan', session: S, seq: 3, content: 'secret plan' },
      { kind: 'task_list', session: S, seq: 4, content: '- [ ] x' },
    ])
    expect(state.items).toEqual([
      { type: 'error', message: 'boom' },
      { type: 'divider', label: 'context compacted' },
    ])
  })
})

describe('foldPrompt / foldHistory', () => {
  it('foldPrompt closes the open bubble and appends a user bubble', () => {
    let state = fold([{ kind: 'text_delta', session: S, seq: 1, text: 'partial' }])
    state = foldPrompt(state, 'next question')
    expect(state.items.map((i) => i.type)).toEqual(['assistant', 'user'])
    expect((state.items[0] as AssistantBubble).streaming).toBe(false)
  })

  it('folds a history replay into user bubbles + closed assistant bubbles', () => {
    const records: AssistantHistoryRecord[] = [
      { dir: 'in', payload: { kind: 'prompt', session: S, content: [{ type: 'text', text: 'hello' }] } },
      { dir: 'out', payload: { kind: 'text_delta', session: S, seq: 1, text: 'hi ' } },
      { dir: 'out', payload: { kind: 'text_delta', session: S, seq: 2, text: 'there' } },
      { dir: 'out', payload: { kind: 'done', session: S, seq: 3 } },
    ]
    const state = foldHistory(records)
    expect(state.items).toEqual([
      { type: 'user', text: 'hello' },
      { type: 'assistant', text: 'hi there', reasoning: '', streaming: false },
    ])
  })

  it('closes a bubble a truncated log left streaming', () => {
    const state = foldHistory([
      { dir: 'out', payload: { kind: 'text_delta', session: S, seq: 1, text: 'cut off' } },
    ])
    expect((state.items[0] as AssistantBubble).streaming).toBe(false)
  })
})

describe('prettyJson', () => {
  it('pretty-prints JSON and passes non-JSON through', () => {
    expect(prettyJson('{"a":1}')).toBe('{\n  "a": 1\n}')
    expect(prettyJson('not json')).toBe('not json')
  })
})
