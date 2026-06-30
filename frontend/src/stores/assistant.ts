// Pinia store for the embedded AI study assistant (issue #20). Owns the list of
// chat sessions and the open one (`current`), and drives the agent loop through
// the backend: posting a message runs the loop until it answers, pauses for a
// mutating-tool approval, or hits the iteration cap; `respond` resolves a pause.
// Thin wrapper over `api.assistant` — the loop itself lives server-side.

import { defineStore } from 'pinia'
import { computed, ref } from 'vue'
import { api } from '../api'
import type { AssistantSession, AssistantSessionSummary } from '../types'

export const useAssistantStore = defineStore('assistant', () => {
  const sessions = ref<AssistantSessionSummary[]>([])
  const current = ref<AssistantSession | null>(null)
  const loading = ref(false)
  const sending = ref(false)
  const error = ref<string | null>(null)

  /** The calls awaiting an approve/deny decision in the open session. */
  const pending = computed(() => current.value?.pending_approvals ?? [])
  const awaitingApproval = computed(() => current.value?.awaiting_approval ?? false)

  async function _run<T>(fn: () => Promise<T>, flag = loading): Promise<T> {
    flag.value = true
    error.value = null
    try {
      return await fn()
    } catch (e) {
      error.value = String((e as Error)?.message ?? e)
      throw e
    } finally {
      flag.value = false
    }
  }

  /** Refresh the session sidebar. */
  async function refresh() {
    sessions.value = await _run(() => api.assistant.listSessions())
    return sessions.value
  }

  /** Open a session (loading its transcript) into `current`. */
  async function open(id: number) {
    current.value = await _run(() => api.assistant.getSession(id))
    return current.value
  }

  /** Start a new session and open it. */
  async function create(title?: string, model?: string) {
    const session = await _run(() => api.assistant.createSession(title, model))
    current.value = session
    await refresh()
    return session
  }

  /** Delete a session; clears `current` if it was the open one. */
  async function remove(id: number) {
    await _run(() => api.assistant.deleteSession(id))
    if (current.value?.id === id) current.value = null
    sessions.value = sessions.value.filter((s) => s.id !== id)
  }

  /** Post a message into the open session and run the loop. */
  async function sendMessage(text: string) {
    if (!current.value) return
    const id = current.value.id
    current.value = await _run(() => api.assistant.send(id, text), sending)
    await refresh()
    return current.value
  }

  /** Resolve the pending approval with a per-call decision map, then continue. */
  async function respond(decisions: Record<string, boolean>) {
    if (!current.value) return
    const id = current.value.id
    current.value = await _run(() => api.assistant.respond(id, decisions), sending)
    await refresh()
    return current.value
  }

  /** Approve (or deny) every pending call in one go. */
  async function resolveAll(approved: boolean) {
    const decisions: Record<string, boolean> = {}
    for (const call of pending.value) decisions[call.id] = approved
    return respond(decisions)
  }

  return {
    sessions,
    current,
    loading,
    sending,
    error,
    pending,
    awaitingApproval,
    refresh,
    open,
    create,
    remove,
    sendMessage,
    respond,
    resolveAll,
  }
})
