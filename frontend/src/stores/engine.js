// Pinia store wrapping the `/api/engine/analyse` WebSocket: it owns the socket,
// folds the streamed events into reactive analysis state, and exposes the small
// control surface (analyse / stop / reconfigure) the UI drives.
//
// The socket factory is injectable (`_setSocketFactory`) so the stream-handling
// logic can be unit-tested with a fake WebSocket and no real backend.

import { defineStore } from 'pinia'
import { ref, computed, shallowRef } from 'vue'
import { parseEngineMessage, reduceInfo, sortedLines } from '../lib/engineStream.js'

const ENDPOINT = '/api/engine/analyse'

/** ws:// URL for the engine endpoint, derived from the page origin. */
function defaultWsUrl() {
  const proto = window.location.protocol === 'https:' ? 'wss:' : 'ws:'
  return `${proto}//${window.location.host}${ENDPOINT}`
}

export const useEngineStore = defineStore('engine', () => {
  // 'idle' | 'connecting' | 'ready' | 'analysing' | 'error' | 'closed'
  const status = ref('idle')
  const engineName = ref(null)
  const error = ref(null)
  const depth = ref(null)
  const nps = ref(null)
  const lines = ref([])
  // Last terminal bestmove; `seq` makes successive identical moves observable.
  const bestMove = shallowRef(null)

  // Engine options surfaced in the UI; sent with every search.
  const multipv = ref(1)
  const threads = ref(1)
  const hash = ref(16)

  const ready = computed(() => status.value === 'ready' || status.value === 'analysing')
  const analysing = computed(() => status.value === 'analysing')

  let socket = null
  let makeSocket = (url) => new WebSocket(url)
  let lineMap = new Map()
  let bestSeq = 0
  let pending = false
  let current = null // last analyse request: { fen, limits, options }

  function _refreshLines() {
    lines.value = sortedLines(lineMap)
    const top = lines.value[0]
    if (top) {
      depth.value = top.depth
      nps.value = top.nps
    }
  }

  function _onMessage(ev) {
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

  function _send(obj) {
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
    if (_send({ type: 'analyse', fen: current.fen, limits: current.limits, options })) {
      status.value = 'analysing'
      return true
    }
    return false
  }

  /** Start (or restart) a search on `fen`. Buffers until the socket opens. */
  function analyse(fen, { limits = {}, options = {} } = {}) {
    current = { fen, limits, options }
    if (!_sendAnalyse()) pending = true
  }

  /** Stop the current search; the engine still emits a final bestmove. */
  function stop() {
    _send({ type: 'stop' })
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
  function _setSocketFactory(fn) {
    makeSocket = fn
  }

  return {
    status,
    engineName,
    error,
    depth,
    nps,
    lines,
    bestMove,
    multipv,
    threads,
    hash,
    ready,
    analysing,
    connect,
    analyse,
    stop,
    reconfigure,
    disconnect,
    _setSocketFactory,
  }
})
