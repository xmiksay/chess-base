import { describe, it, expect, beforeEach } from 'vitest'
import { setActivePinia, createPinia } from 'pinia'
import { useEngineStore } from './engine'

// A frame sent over the fake socket (analyse / stop request).
interface SentFrame {
  type: string
  fen: string
  limits: unknown
  options: Record<string, string>
}

// Minimal WebSocket double: records sent frames and lets tests drive the
// open/message/close lifecycle by hand.
class FakeWebSocket {
  static last: FakeWebSocket
  readyState: number
  sent: SentFrame[]
  onopen?: () => void
  onmessage?: (ev: { data: string }) => void
  onclose?: () => void
  onerror?: () => void
  constructor() {
    this.readyState = 0 // CONNECTING
    this.sent = []
    FakeWebSocket.last = this
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

function freshStore() {
  const store = useEngineStore()
  store._setSocketFactory(() => new FakeWebSocket() as unknown as WebSocket)
  return store
}

describe('engine store', () => {
  beforeEach(() => setActivePinia(createPinia()))

  it('becomes ready on the ready frame', () => {
    const engine = freshStore()
    engine.connect()
    FakeWebSocket.last.open()
    expect(engine.status).toBe('connecting')
    FakeWebSocket.last.emit({ type: 'ready', name: 'Stockfish 16' })
    expect(engine.engineName).toBe('Stockfish 16')
    expect(engine.ready).toBe(true)
  })

  it('sends an analyse frame with managed options', () => {
    const engine = freshStore()
    engine.multipv = 3
    engine.threads = 2
    engine.hash = 64
    engine.connect()
    FakeWebSocket.last.open()
    engine.analyse('startfen', { limits: { depth: 20 } })
    const frame = FakeWebSocket.last.sent.at(-1)
    expect(frame).toEqual({
      type: 'analyse',
      fen: 'startfen',
      limits: { depth: 20 },
      options: { Threads: '2', Hash: '64', MultiPV: '3' },
    })
    expect(engine.status).toBe('analysing')
  })

  it('records the searched fen so the UI can format the eval against it', () => {
    const engine = freshStore()
    engine.connect()
    FakeWebSocket.last.open()
    expect(engine.analysedFen).toBeNull()
    engine.analyse('searched-fen', {})
    expect(engine.analysedFen).toBe('searched-fen')
    // A later search retargets it; a stale board fen must never drive the sign.
    engine.analyse('next-fen', {})
    expect(engine.analysedFen).toBe('next-fen')
  })

  it('buffers an analyse issued before the socket opens', () => {
    const engine = freshStore()
    engine.connect()
    engine.analyse('fen-1', {})
    expect(FakeWebSocket.last.sent).toHaveLength(0) // not open yet
    FakeWebSocket.last.open()
    expect(FakeWebSocket.last.sent.at(-1)!.fen).toBe('fen-1')
  })

  it('folds info frames into sorted lines and tracks depth/nps', () => {
    const engine = freshStore()
    engine.connect()
    FakeWebSocket.last.open()
    FakeWebSocket.last.emit({ type: 'info', multipv: 2, depth: 18, score: { type: 'cp', value: -5 }, pv: ['e7e5'] })
    FakeWebSocket.last.emit({ type: 'info', multipv: 1, depth: 18, nps: 900000, score: { type: 'cp', value: 25 }, pv: ['e2e4'] })
    expect(engine.lines.map((l) => l.multipv)).toEqual([1, 2])
    expect(engine.depth).toBe(18)
    expect(engine.nps).toBe(900000)
  })

  it('records the terminal bestmove and returns to ready', () => {
    const engine = freshStore()
    engine.connect()
    FakeWebSocket.last.open()
    engine.analyse('fen', { limits: { movetime_ms: 100 } })
    FakeWebSocket.last.emit({ type: 'bestmove', best_move: 'e2e4', ponder: 'e7e5' })
    expect(engine.bestMove!.move).toBe('e2e4')
    expect(engine.bestMove!.ponder).toBe('e7e5')
    expect(engine.status).toBe('ready')
  })

  it('reconfigure re-issues the current search', () => {
    const engine = freshStore()
    engine.connect()
    FakeWebSocket.last.open()
    engine.analyse('fen', {})
    engine.multipv = 4
    engine.reconfigure()
    const frame = FakeWebSocket.last.sent.at(-1)
    expect(frame!.options.MultiPV).toBe('4')
  })

  it('threads planline frames into plans and derives dimmed shapes on hover', () => {
    const engine = freshStore()
    engine.connect()
    FakeWebSocket.last.open()
    FakeWebSocket.last.emit({
      type: 'planline',
      multipv: 1,
      pv: ['g1f3'],
      trajectories: [{ piece: 'N', squares: ['g1', 'f3', 'g5'] }],
    })
    FakeWebSocket.last.emit({
      type: 'planline',
      multipv: 2,
      pv: ['e2e4'],
      trajectories: [{ piece: 'P', squares: ['e2', 'e4'] }],
    })
    expect(engine.plans.map((p) => p.multipv)).toEqual([1, 2])
    // No active line ⇒ both lines drawn at full opacity.
    expect(engine.shapes.every((s) => !s.brush?.endsWith('d'))).toBe(true)
    engine.setActiveLine(1)
    expect(engine.shapes.find((s) => s.orig === 'e2')?.brush).toBe('plan2d')
    expect(engine.shapes.find((s) => s.orig === 'g1')?.brush).toBe('plan1')
  })

  it('clears plans when a new search starts', () => {
    const engine = freshStore()
    engine.connect()
    FakeWebSocket.last.open()
    FakeWebSocket.last.emit({
      type: 'planline',
      multipv: 1,
      pv: ['g1f3'],
      trajectories: [{ piece: 'N', squares: ['g1', 'f3'] }],
    })
    expect(engine.plans).toHaveLength(1)
    engine.analyse('newfen', {})
    expect(engine.plans).toHaveLength(0)
    expect(engine.shapes).toHaveLength(0)
  })

  it('surfaces error frames', () => {
    const engine = freshStore()
    engine.connect()
    FakeWebSocket.last.open()
    FakeWebSocket.last.emit({ type: 'error', message: 'bad fen' })
    expect(engine.error).toBe('bad fen')
  })
})
