// Pinia store wrapping the `/api/engine/analyse` WebSocket: it owns the socket,
// folds the streamed events into reactive analysis state, and exposes the small
// control surface (analyse / stop / reconfigure) the UI drives.
//
// The socket factory is injectable (`_setSocketFactory`) so the stream-handling
// logic can be unit-tested with a fake WebSocket and no real backend.

import { defineStore } from 'pinia'
import { ref, computed, shallowRef } from 'vue'
import { parseEngineMessage, reduceInfo, sortedLines } from '../lib/engineStream'
import { plansToShapes } from '../lib/plansToShapes'
import type { BestMove, EngineLine, PlanLine } from '../types'

const ENDPOINT = '/api/engine/analyse'

/** ws:// URL for the engine endpoint, derived from the page origin. */
function defaultWsUrl() {
  const proto = window.location.protocol === 'https:' ? 'wss:' : 'ws:'
  return `${proto}//${window.location.host}${ENDPOINT}`
}

export const useEngineStore = defineStore('engine', () => {
  const status = ref<'idle' | 'connecting' | 'ready' | 'analysing' | 'error' | 'closed'>('idle')
  const engineName = ref<string | null>(null)
  const error = ref<string | null>(null)
  const depth = ref<number | null>(null)
  const nps = ref<number | null>(null)
  const lines = ref<EngineLine[]>([])
  // Per-piece trajectories per MultiPV line, for the Plans overlay (#60).
  const plans = ref<PlanLine[]>([])
  // MultiPV of the line the UI is hovering; drives which plan is highlighted.
  const activeLine = ref<number | null>(null)
  // Last terminal bestmove; `seq` makes successive identical moves observable.
  const bestMove = shallowRef<BestMove | null>(null)

  // Engine options surfaced in the UI; sent with every search.
  const multipv = ref(1)
  const threads = ref(1)
  const hash = ref(16)

  const ready = computed(() => status.value === 'ready' || status.value === 'analysing')
  const analysing = computed(() => status.value === 'analysing')
  // Chessground auto-shapes for the active overlay; the active line keeps full
  // opacity, the rest are dimmed. Empty until a `planline` frame arrives.
  const shapes = computed(() => plansToShapes(plans.value, { active: activeLine.value, labels: true }))

  let socket: WebSocket | null = null
  let makeSocket: (url: string) => WebSocket = (url) => new WebSocket(url)
  let lineMap = new Map<number, EngineLine>()
  let planMap = new Map<number, PlanLine>()
  let bestSeq = 0
  let pending = false
  // last analyse request
  let current: { fen: string; limits: Record<string, unknown>; options: Record<string, string> } | null = null

  function _refreshLines() {
    lines.value = sortedLines(lineMap)
    const top = lines.value[0]
    if (top) {
      depth.value = top.depth
      nps.value = top.nps
    }
  }

  function _refreshPlans() {
    plans.value = [...planMap.values()].sort((a, b) => a.multipv - b.multipv)
  }

  function _onMessage(ev: MessageEvent) {
    const msg = parseEngineMessage(ev.data)
    if (!msg) return
    switch (msg.type) {
      case 'ready':
        engineName.value = msg.name
        if (status.value === 'connecting') status.value = 'ready'
        break
      case 'info':
        reduceInfo(lineMap, msg)
        _refreshLines()
        break
      case 'planline': {
        const idx = msg.multipv ?? 1
        planMap.set(idx, {
          multipv: idx,
          depth: msg.depth ?? null,
          score: msg.score ?? null,
          pv: msg.pv ?? [],
          trajectories: msg.trajectories ?? [],
        })
        _refreshPlans()
        break
      }
      case 'bestmove':
        bestMove.value = { move: msg.best_move, ponder: msg.ponder ?? null, seq: ++bestSeq }
        if (status.value === 'analysing') status.value = 'ready'
        break
      case 'error':
        error.value = msg.message
        break
    }
  }

  function connect() {
    if (socket && (status.value === 'connecting' || ready.value)) return
    error.value = null
    status.value = 'connecting'
    try {
      socket = makeSocket(defaultWsUrl())
    } catch (e) {
      status.value = 'error'
      error.value = String(e)
      return
    }
    socket.onopen = () => {
      if (pending) {
        pending = false
        _sendAnalyse()
      }
    }
    socket.onmessage = _onMessage
    socket.onerror = () => {
      error.value = 'engine connection error'
    }
    socket.onclose = () => {
      socket = null
      if (status.value !== 'error') status.value = 'closed'
    }
  }

  function _send(obj: unknown) {
    if (!socket || socket.readyState !== 1) return false
    socket.send(JSON.stringify(obj))
    return true
  }

  function _sendAnalyse() {
    if (!current) return false
    // Caller options first; managed options (Threads/Hash/MultiPV) win.
    const options = {
      Threads: String(threads.value),
      Hash: String(hash.value),
      ...current.options,
      MultiPV: String(multipv.value),
    }
    lineMap = new Map()
    lines.value = []
    planMap = new Map()
    plans.value = []
    if (_send({ type: 'analyse', fen: current.fen, limits: current.limits, options })) {
      status.value = 'analysing'
      return true
    }
    return false
  }

  /** Start (or restart) a search on `fen`. Buffers until the socket opens. */
  function analyse(
    fen: string,
    { limits = {}, options = {} }: { limits?: Record<string, unknown>; options?: Record<string, string> } = {},
  ) {
    current = { fen, limits, options }
    if (!_sendAnalyse()) pending = true
  }

  /** Stop the current search; the engine still emits a final bestmove. */
  function stop() {
    _send({ type: 'stop' })
  }

  /** Highlight one MultiPV line's plan (dimming the rest); null clears it. */
  function setActiveLine(multipv: number | null) {
    activeLine.value = multipv
  }

  /** Re-issue the current search after an option change (MultiPV/Threads/Hash). */
  function reconfigure() {
    if (current && status.value === 'analysing') _sendAnalyse()
  }

  function disconnect() {
    if (socket) {
      try {
        socket.close()
      } catch {
        /* already gone */
      }
    }
    socket = null
    pending = false
    status.value = 'idle'
  }

  /** Test seam: override the WebSocket constructor. */
  function _setSocketFactory(fn: (url: string) => WebSocket) {
    makeSocket = fn
  }

  return {
    status,
    engineName,
    error,
    depth,
    nps,
    lines,
    plans,
    activeLine,
    shapes,
    bestMove,
    multipv,
    threads,
    hash,
    ready,
    analysing,
    connect,
    analyse,
    stop,
    setActiveLine,
    reconfigure,
    disconnect,
    _setSocketFactory,
  }
})
