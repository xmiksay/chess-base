// Pinia store holding the board/game state, backed by a chess.js instance for
// legality. Drives both the analysis board (free movement) and play-vs-engine.
//
// The full game is kept as a flat line of plies; a `ply` cursor selects which
// position the board (and engine analysis, via `fen`) shows. Navigation moves
// the cursor without mutating the line; playing a move at a non-tip ply
// truncates the line past the cursor (this flat view has no variations).

import { defineStore } from 'pinia'
import { ref, computed } from 'vue'
import { Chess } from 'chess.js'
import { STARTPOS_FEN } from '../lib/fen'
import type { BoardMove, Color, Dests, Square } from '../types'

interface Ply {
  san: string
  from: Square
  to: Square
  fen: string // position after this move
}

export const useGameStore = defineStore('game', () => {
  let chess: Chess = new Chess() // positioned at the current ply

  const startFen = ref(chess.fen()) // position before the first move
  const line = ref<Ply[]>([]) // full game line
  const ply = ref(0) // cursor: 0 = start, n = after the nth move
  const fen = ref(chess.fen())
  const orientation = ref<Color>('white') // board orientation
  const mode = ref<'analyse' | 'play'>('analyse') // 'analyse' | 'play'
  const playColor = ref<Color>('white') // human's color in play mode

  /** SAN moves played, as a flat list — drives the move-list notation panel. */
  const history = computed<string[]>(() => line.value.map((m) => m.san))

  /** FEN of the position after `n` plies (`n` is clamped to the line). */
  function fenAt(n: number): string {
    return n <= 0 ? startFen.value : line.value[n - 1].fen
  }

  const atStart = computed(() => ply.value <= 0)
  const atEnd = computed(() => ply.value >= line.value.length)

  /** `[from, to]` of the move leading to the current ply, for board highlight. */
  const lastMove = computed<[Square, Square] | null>(() => {
    if (ply.value <= 0) return null
    const m = line.value[ply.value - 1]
    return m ? [m.from, m.to] : null
  })

  const turnColor = computed(() => (fen.value && chess.turn() === 'b' ? 'black' : 'white'))

  const gameOver = computed(() => {
    void fen.value // reactive dep
    return chess.isGameOver()
  })

  /** Game outcome once over: 'white' | 'black' (winner) | 'draw' | null. */
  const result = computed(() => {
    void fen.value // reactive dep
    if (!chess.isGameOver()) return null
    if (chess.isCheckmate()) return chess.turn() === 'w' ? 'black' : 'white'
    return 'draw'
  })

  /** Legal-move map for chessground: from-square → array of destination squares. */
  const legalDests = computed<Dests>(() => {
    void fen.value // reactive dep
    const map = new Map<string, string[]>()
    for (const m of chess.moves({ verbose: true })) {
      const arr = map.get(m.from)
      if (arr) arr.push(m.to)
      else map.set(m.from, [m.to])
    }
    return map
  })

  /** Apply a board move {from,to,promotion?}; returns the SAN or null if illegal. */
  function playMove({ from, to, promotion = 'q' }: BoardMove): string | null {
    let move
    try {
      move = chess.move({ from, to, promotion })
    } catch {
      return null
    }
    if (!move) return null
    // Branch off the current cursor: drop any line past it, append this move.
    line.value = line.value.slice(0, ply.value)
    line.value.push({ san: move.san, from: move.from, to: move.to, fen: chess.fen() })
    ply.value = line.value.length
    fen.value = chess.fen()
    return move.san
  }

  /** Apply an engine move given in UCI long-algebraic form. */
  function playUci(uci: string): string | null {
    if (typeof uci !== 'string' || uci.length < 4) return null
    return playMove({
      from: uci.slice(0, 2),
      to: uci.slice(2, 4),
      promotion: uci.length > 4 ? uci[4].toLowerCase() : 'q',
    })
  }

  /** Move the cursor to ply `n` (clamped); the board/engine follow `fen`. */
  function goto(n: number) {
    const target = Math.max(0, Math.min(n, line.value.length))
    if (target === ply.value) return
    chess = new Chess(fenAt(target))
    ply.value = target
    fen.value = chess.fen()
  }

  const next = () => goto(ply.value + 1)
  const prev = () => goto(ply.value - 1)
  const first = () => goto(0)
  const last = () => goto(line.value.length)

  function reset(fenString: string = STARTPOS_FEN) {
    chess = new Chess(fenString)
    startFen.value = chess.fen()
    line.value = []
    ply.value = 0
    fen.value = chess.fen()
  }

  /** Remove the last move of the line; the cursor follows back to the new tip. */
  function undo() {
    if (!line.value.length) return
    line.value = line.value.slice(0, -1)
    const target = Math.min(ply.value, line.value.length)
    chess = new Chess(fenAt(target))
    ply.value = target
    fen.value = chess.fen()
  }

  /** Load an arbitrary position as a fresh start; returns false on invalid FEN. */
  function setFen(newFen: string): boolean {
    let next
    try {
      next = new Chess(newFen)
    } catch {
      return false
    }
    chess = next
    startFen.value = chess.fen()
    line.value = []
    ply.value = 0
    fen.value = chess.fen()
    return true
  }

  return {
    fen,
    history,
    ply,
    atStart,
    atEnd,
    lastMove,
    orientation,
    mode,
    playColor,
    turnColor,
    gameOver,
    result,
    legalDests,
    playMove,
    playUci,
    goto,
    next,
    prev,
    first,
    last,
    reset,
    undo,
    setFen,
  }
})
