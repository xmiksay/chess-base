// Pure helpers for the assistant WebSocket stream (issue #198).
//
// The backend relays the entanglement engine's `kind`-tagged snake_case events
// inside a `type`-tagged envelope (see src/server/assistant_ws/protocol.rs).
// This module parses envelope frames and folds engine events into a
// `TranscriptState` the view renders — framework-free and unit-testable, the
// same seam pattern as lib/engineStream.ts.

import type {
  AgentState,
  AssistantHistoryRecord,
  AssistantOutEvent,
  AssistantQuestion,
  AssistantServerFrame,
  ContentPart,
} from '../types'

// --- transcript model --------------------------------------------------------

export interface UserBubble {
  type: 'user'
  text: string
}

/** A streamed assistant segment; `streaming` shows the live cursor. */
export interface AssistantBubble {
  type: 'assistant'
  text: string
  reasoning: string
  streaming: boolean
}

export interface ToolChip {
  type: 'tool'
  requestId: string
  tool: string
  state: 'running' | 'done' | 'error'
}

/** A mutating-tool approval request; `resolved` null while it awaits a decision. */
export interface ApprovalCard {
  type: 'approval'
  requestId: string
  tool: string
  prettyInput: string
  resolved: 'approved' | 'rejected' | null
}

export interface QuestionCard {
  type: 'question'
  requestId: string
  questions: AssistantQuestion[]
  resolved: boolean
}

export interface Divider {
  type: 'divider'
  label: string
}

export interface ErrorNotice {
  type: 'error'
  message: string
}

export type TranscriptItem =
  | UserBubble
  | AssistantBubble
  | ToolChip
  | ApprovalCard
  | QuestionCard
  | Divider
  | ErrorNotice

/** Session token totals, accumulated from per-round `usage` events. */
export interface UsageTotals {
  input_tokens: number
  output_tokens: number
  cost_usd: number | null
}

export interface TranscriptState {
  items: TranscriptItem[]
  status: AgentState | null
  usage: UsageTotals | null
}

export function emptyTranscript(): TranscriptState {
  return { items: [], status: null, usage: null }
}

// --- parsing -----------------------------------------------------------------

/**
 * Parse one raw WebSocket text frame into a server envelope frame.
 * Returns null for frames that aren't a JSON object carrying a string `type`.
 */
export function parseServerFrame(raw: string): AssistantServerFrame | null {
  let msg
  try {
    msg = JSON.parse(raw)
  } catch {
    return null
  }
  if (!msg || typeof msg !== 'object' || typeof msg.type !== 'string') return null
  return msg as AssistantServerFrame
}

/** Pretty-print a tool `input` JSON string; a non-JSON payload passes through. */
export function prettyJson(input: string): string {
  try {
    return JSON.stringify(JSON.parse(input), null, 2)
  } catch {
    return input
  }
}

/** The plain text of a prompt's content parts. */
export function contentText(content: ContentPart[] | undefined): string {
  return (content ?? [])
    .filter((part) => typeof part.text === 'string')
    .map((part) => part.text)
    .join('')
}

// --- reducer -----------------------------------------------------------------

function withItems(state: TranscriptState, items: TranscriptItem[]): TranscriptState {
  return { ...state, items }
}

/** Mark any trailing streaming assistant bubble as closed. */
function closeBubble(state: TranscriptState): TranscriptState {
  const last = state.items.at(-1)
  if (last?.type !== 'assistant' || !last.streaming) return state
  return withItems(state, [...state.items.slice(0, -1), { ...last, streaming: false }])
}

/** Append delta text to the open assistant bubble, creating one if needed. */
function appendDelta(
  state: TranscriptState,
  field: 'text' | 'reasoning',
  text: string,
): TranscriptState {
  const last = state.items.at(-1)
  if (last?.type === 'assistant' && last.streaming) {
    const bubble = { ...last, [field]: last[field] + text }
    return withItems(state, [...state.items.slice(0, -1), bubble])
  }
  const bubble: AssistantBubble = { type: 'assistant', text: '', reasoning: '', streaming: true }
  bubble[field] = text
  return withItems(state, [...state.items, bubble])
}

function findChip(items: TranscriptItem[], requestId: string): number {
  return items.findIndex((i) => i.type === 'tool' && i.requestId === requestId)
}

function findCard(items: TranscriptItem[], requestId: string): number {
  return items.findIndex((i) => i.type === 'approval' && i.requestId === requestId)
}

/** Ensure a running tool chip exists for `requestId` (closing the open bubble). */
function ensureChip(state: TranscriptState, requestId: string, tool: string): TranscriptState {
  if (findChip(state.items, requestId) >= 0) return state
  const closed = closeBubble(state)
  const chip: ToolChip = { type: 'tool', requestId, tool, state: 'running' }
  return withItems(closed, [...closed.items, chip])
}

function setChipState(
  state: TranscriptState,
  requestId: string,
  chipState: ToolChip['state'],
): TranscriptState {
  const idx = findChip(state.items, requestId)
  if (idx < 0) return state
  const items = [...state.items]
  items[idx] = { ...(items[idx] as ToolChip), state: chipState }
  return withItems(state, items)
}

/** Resolve an approval card (no-op when absent or already resolved). */
export function resolveApproval(
  state: TranscriptState,
  requestId: string,
  resolution: 'approved' | 'rejected',
): TranscriptState {
  const idx = findCard(state.items, requestId)
  if (idx < 0) return state
  const card = state.items[idx] as ApprovalCard
  if (card.resolved) return state
  const items = [...state.items]
  items[idx] = { ...card, resolved: resolution }
  return withItems(state, items)
}

/** Mark a question card answered (no-op when absent). */
export function resolveQuestion(state: TranscriptState, requestId: string): TranscriptState {
  const idx = state.items.findIndex((i) => i.type === 'question' && i.requestId === requestId)
  if (idx < 0) return state
  const items = [...state.items]
  items[idx] = { ...(items[idx] as QuestionCard), resolved: true }
  return withItems(state, items)
}

/** Fold a user prompt in: closes the open bubble and appends a user bubble. */
export function foldPrompt(state: TranscriptState, text: string): TranscriptState {
  const closed = closeBubble(state)
  return withItems(closed, [...closed.items, { type: 'user', text }])
}

/**
 * Fold one engine event into the transcript. Pure: returns a new state (the
 * items array and any changed item are copied). Unknown/uninteresting kinds
 * (`plan`, `task_list`, lifecycle events) fall through unchanged.
 */
export function foldEvent(state: TranscriptState, ev: AssistantOutEvent): TranscriptState {
  switch (ev.kind) {
    case 'text_delta':
      return appendDelta(state, 'text', ev.text)
    case 'reasoning_delta':
      return appendDelta(state, 'reasoning', ev.text)
    case 'tool_call_delta':
    case 'tool_call':
      return ensureChip(state, ev.request_id, ev.tool)
    case 'tool_request': {
      const closed = closeBubble(state)
      const card: ApprovalCard = {
        type: 'approval',
        requestId: ev.request_id,
        tool: ev.tool,
        prettyInput: prettyJson(ev.input),
        resolved: null,
      }
      return withItems(closed, [...closed.items, card])
    }
    case 'tool_exec':
      // An exec after an approval card means the request was approved.
      return ensureChip(resolveApproval(state, ev.request_id, 'approved'), ev.request_id, ev.tool)
    case 'tool_output': {
      // A tool_output with an unresolved approval card and no exec is the
      // denial result: resolve the card rejected and flag the chip.
      const cardIdx = findCard(state.items, ev.request_id)
      const card = cardIdx >= 0 ? (state.items[cardIdx] as ApprovalCard) : null
      if (card && !card.resolved) {
        return setChipState(resolveApproval(state, ev.request_id, 'rejected'), ev.request_id, 'error')
      }
      return setChipState(state, ev.request_id, 'done')
    }
    case 'user_question': {
      const closed = closeBubble(state)
      const card: QuestionCard = {
        type: 'question',
        requestId: ev.request_id,
        questions: ev.questions ?? [],
        resolved: false,
      }
      return withItems(closed, [...closed.items, card])
    }
    case 'status':
      return { ...state, status: ev.state }
    case 'usage': {
      const prev = state.usage ?? { input_tokens: 0, output_tokens: 0, cost_usd: null }
      return {
        ...state,
        usage: {
          input_tokens: prev.input_tokens + ev.input_tokens,
          output_tokens: prev.output_tokens + ev.output_tokens,
          cost_usd:
            ev.cost_usd == null ? prev.cost_usd : (prev.cost_usd ?? 0) + ev.cost_usd,
        },
      }
    }
    case 'error': {
      const closed = closeBubble(state)
      return withItems(closed, [...closed.items, { type: 'error', message: ev.message }])
    }
    case 'done':
      return closeBubble(state)
    case 'compacted': {
      const closed = closeBubble(state)
      return withItems(closed, [...closed.items, { type: 'divider', label: 'context compacted' }])
    }
    default:
      return state
  }
}

/**
 * Fold a `history` frame's records into a fresh transcript: `in` prompt
 * records become user bubbles, `out` events run through the same reducer, and
 * any bubble left streaming by a truncated log is closed at the end.
 */
export function foldHistory(records: AssistantHistoryRecord[]): TranscriptState {
  let state = emptyTranscript()
  for (const record of records) {
    if (record.dir === 'in') {
      const msg = record.payload
      if (msg.kind === 'prompt') state = foldPrompt(state, contentText(msg.content))
    } else {
      state = foldEvent(state, record.payload as AssistantOutEvent)
    }
  }
  return closeBubble(state)
}
