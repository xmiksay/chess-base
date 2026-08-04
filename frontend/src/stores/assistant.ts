// Pinia store for the embedded AI assistant (issue #198): a streaming
// WebSocket client over `/api/assistant/ws`. Owns the socket (connect /
// capped-backoff reconnect / teardown, mirroring stores/engine.ts), the
// session sidebar, and the open conversation's transcript — every engine
// event for the current root runs through the pure lib/assistantStream fold.
//
// The socket factory is injectable (`_setSocketFactory`) so tests drive the
// stream with a fake WebSocket and no backend.

import { defineStore } from 'pinia'
import { computed, ref } from 'vue'
import {
  emptyTranscript,
  foldEvent,
  foldHistory,
  foldPrompt,
  parseServerFrame,
  resolveApproval,
  resolveQuestion,
} from '../lib/assistantStream'
import type { ApprovalCard, QuestionCard, TranscriptState } from '../lib/assistantStream'
import type { ModelChoice } from '../lib/providers'
import type {
  AgentSessionSummary,
  AssistantClientFrame,
  AssistantInMsg,
  AssistantOutEvent,
  AssistantServerFrame,
} from '../types'

const ENDPOINT = '/api/assistant/ws'
/** Reconnect backoff: 1s, 2s, 4s, … capped at 15s. */
const BACKOFF_BASE_MS = 1000
const BACKOFF_CAP_MS = 15000
/** Consecutive failed connects before the UI shows "assistant unavailable". */
const UNAVAILABLE_AFTER = 2

function defaultWsUrl() {
  const proto = window.location.protocol === 'https:' ? 'wss:' : 'ws:'
  return `${proto}//${window.location.host}${ENDPOINT}`
}

export const useAssistantStore = defineStore('assistant', () => {
  const connected = ref(false)
  // The agent engine is off (WS refused repeatedly) or no provider is
  // configured — the view shows a callout linking to Settings.
  const unavailable = ref(false)
  const sessions = ref<AgentSessionSummary[]>([])
  const currentRoot = ref<string | null>(null)
  const transcript = ref<TranscriptState>(emptyTranscript())
  const throttled = ref(false)
  // Events were dropped (broadcast lag or a gapped log): the transcript may be
  // incomplete until the session is reopened.
  const gapped = ref(false)
  const error = ref<string | null>(null)

  const pendingApprovals = computed(
    () =>
      transcript.value.items.filter(
        (i): i is ApprovalCard => i.type === 'approval' && i.resolved === null,
      ),
  )
  const pendingQuestions = computed(
    () =>
      transcript.value.items.filter(
        (i): i is QuestionCard => i.type === 'question' && !i.resolved,
      ),
  )
  /** Whether the open conversation is mid-turn (drives the Stop button). */
  const streaming = computed(() => {
    const last = transcript.value.items.at(-1)
    if (last?.type === 'assistant' && last.streaming) return true
    return transcript.value.status === 'thinking' || transcript.value.status === 'working'
  })

  const current = computed(
    () => sessions.value.find((s) => s.root_session_id === currentRoot.value) ?? null,
  )

  let socket: WebSocket | null = null
  let makeSocket: (url: string) => WebSocket = (url) => new WebSocket(url)
  let closedByUs = false
  let attempts = 0
  let retryTimer: ReturnType<typeof setTimeout> | null = null
  // The prompt sent with a pending `new` frame, echoed into the transcript
  // once `created` names the root (the engine doesn't re-broadcast prompts).
  let pendingPrompt: string | null = null

  function connect() {
    if (socket) return
    closedByUs = false
    let ws: WebSocket
    try {
      ws = makeSocket(defaultWsUrl())
    } catch (e) {
      error.value = String(e)
      scheduleReconnect()
      return
    }
    socket = ws
    ws.onopen = () => {
      connected.value = true
      unavailable.value = false
      attempts = 0
      error.value = null
      list()
      // Re-enter the open conversation after a reconnect.
      if (currentRoot.value) open(currentRoot.value)
    }
    ws.onmessage = (ev: MessageEvent) => {
      const frame = parseServerFrame(String(ev.data))
      if (frame) handleFrame(frame)
    }
    ws.onerror = () => {
      // onclose follows and owns the reconnect; nothing to record here.
    }
    ws.onclose = () => {
      socket = null
      connected.value = false
      if (!closedByUs) scheduleReconnect()
    }
  }

  function scheduleReconnect() {
    attempts += 1
    if (attempts >= UNAVAILABLE_AFTER) unavailable.value = true
    const delay = Math.min(BACKOFF_BASE_MS * 2 ** (attempts - 1), BACKOFF_CAP_MS)
    if (retryTimer) clearTimeout(retryTimer)
    retryTimer = setTimeout(() => {
      retryTimer = null
      connect()
    }, delay)
  }

  function disconnect() {
    closedByUs = true
    if (retryTimer) {
      clearTimeout(retryTimer)
      retryTimer = null
    }
    if (socket) {
      try {
        socket.close()
      } catch {
        // already gone
      }
      socket = null
    }
    connected.value = false
  }

  function sendFrame(frame: AssistantClientFrame): boolean {
    if (!socket || socket.readyState !== 1) return false
    socket.send(JSON.stringify(frame))
    return true
  }

  function sendIn(msg: AssistantInMsg) {
    sendFrame({ type: 'in', msg })
  }

  // --- frame handling --------------------------------------------------------

  function handleFrame(frame: AssistantServerFrame) {
    switch (frame.type) {
      case 'ping':
        sendFrame({ type: 'pong' })
        break
      case 'sessions':
        sessions.value = frame.items
        break
      case 'created':
        currentRoot.value = frame.root
        gapped.value = false
        transcript.value = pendingPrompt
          ? foldPrompt(emptyTranscript(), pendingPrompt)
          : emptyTranscript()
        pendingPrompt = null
        list()
        break
      case 'history':
        if (frame.root === currentRoot.value) {
          transcript.value = foldHistory(frame.records)
          gapped.value = frame.gapped
        }
        break
      case 'deleted':
        sessions.value = sessions.value.filter((s) => s.root_session_id !== frame.root)
        if (currentRoot.value === frame.root) {
          currentRoot.value = null
          transcript.value = emptyTranscript()
        }
        break
      case 'error':
        error.value = frame.message
        if (frame.code === 'no_provider') unavailable.value = true
        break
      case 'gap':
        gapped.value = true
        break
      case 'throttled':
        throttled.value = frame.active
        break
      case 'out':
        handleOut(frame.ev)
        break
    }
  }

  function handleOut(ev: AssistantOutEvent) {
    // Session-list touches apply whichever conversation the event names.
    if (ev.kind === 'session_meta_changed') {
      sessions.value = sessions.value.map((s) =>
        s.root_session_id === ev.session ? { ...s, name: ev.name ?? s.name } : s,
      )
    } else if (ev.kind === 'session_started' || ev.kind === 'session_ended' || ev.kind === 'session_hibernated') {
      const live = ev.kind === 'session_started'
      sessions.value = sessions.value.map((s) =>
        s.root_session_id === ev.session ? { ...s, live } : s,
      )
    }
    // Transcript fold: current root only (child-session events are ignored v1).
    if (ev.session === currentRoot.value) {
      transcript.value = foldEvent(transcript.value, ev)
    }
  }

  // --- actions ---------------------------------------------------------------

  function list() {
    sendFrame({ type: 'list' })
  }

  /**
   * Start a new conversation; its first turn runs on `prompt`. `modelChoice`
   * is an explicit creation-time provider/model pick (issue #215); omitted ⇒
   * the caller's per-user default.
   */
  function newSession(prompt: string, name: string | null = null, modelChoice: ModelChoice | null = null) {
    error.value = null
    pendingPrompt = prompt
    sendFrame({
      type: 'new',
      prompt,
      name,
      provider: modelChoice?.provider ?? null,
      model: modelChoice?.model ?? null,
    })
  }

  /** Deselect the open conversation (the next send starts a new one). */
  function deselect() {
    currentRoot.value = null
    transcript.value = emptyTranscript()
    gapped.value = false
    error.value = null
  }

  /** Open a conversation: the server replays its history. */
  function open(root: string) {
    error.value = null
    if (currentRoot.value !== root) {
      transcript.value = emptyTranscript()
      gapped.value = false
    }
    currentRoot.value = root
    sendFrame({ type: 'open', root })
  }

  /** Send a user message into the open conversation. */
  function send(text: string) {
    const root = currentRoot.value
    if (!root || !text.trim()) return
    transcript.value = foldPrompt(transcript.value, text)
    sendIn({ kind: 'prompt', session: root, content: [{ type: 'text', text }] })
  }

  function approve(requestId: string, scope: 'once' | 'session' | 'always' = 'once') {
    const root = currentRoot.value
    if (!root) return
    sendIn({
      kind: 'approve',
      session: root,
      request_id: requestId,
      // `once` is the wire default and stays off the frame.
      ...(scope === 'once' ? {} : { scope }),
    })
    transcript.value = resolveApproval(transcript.value, requestId, 'approved')
  }

  function reject(requestId: string) {
    const root = currentRoot.value
    if (!root) return
    sendIn({ kind: 'reject', session: root, request_id: requestId, reason: null })
    transcript.value = resolveApproval(transcript.value, requestId, 'rejected')
  }

  /** Answer a `user_question`: one inner array per question, in order. */
  function answerQuestion(requestId: string, answers: string[][]) {
    const root = currentRoot.value
    if (!root) return
    sendIn({ kind: 'answer_question', session: root, request_id: requestId, answers })
    transcript.value = resolveQuestion(transcript.value, requestId)
  }

  /** Cancel the in-flight turn (the session stays alive). */
  function stop() {
    if (currentRoot.value) sendIn({ kind: 'stop', session: currentRoot.value })
  }

  function rename(root: string, name: string) {
    sendIn({ kind: 'set_session_meta', session: root, name, if_unset: false })
  }

  /** Switch the open conversation's live model/provider (issue #215). */
  function setModel(provider: string, model: string) {
    const root = currentRoot.value
    if (!root) return
    sendIn({ kind: 'set_model', session: root, provider, model })
  }

  function remove(root: string) {
    sendFrame({ type: 'delete', root })
  }

  /** Test seam: override the WebSocket constructor. */
  function _setSocketFactory(fn: (url: string) => WebSocket) {
    makeSocket = fn
  }

  return {
    connected,
    unavailable,
    sessions,
    currentRoot,
    current,
    transcript,
    throttled,
    gapped,
    error,
    pendingApprovals,
    pendingQuestions,
    streaming,
    connect,
    disconnect,
    list,
    newSession,
    deselect,
    open,
    send,
    approve,
    reject,
    answerQuestion,
    stop,
    rename,
    setModel,
    remove,
    _setSocketFactory,
  }
})
