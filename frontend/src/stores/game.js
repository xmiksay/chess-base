// Pinia store holding the board/game state, backed by a chess.js instance for
// legality. Drives both the analysis board (free movement) and play-vs-engine.

import { defineStore } from 'pinia'
import { ref, computed } from 'vue'
import { Chess } from 'chess.js'
import { STARTPOS_FEN } from '../lib/fen.js'

export const useGameStore = defineStore('game', () => {
  let chess = new Chess()

  const fen = ref(chess.fen())
  const history = ref([]) // SAN moves played
  const orientation = ref('white') // board orientation
  const mode = ref('analyse') // 'analyse' | 'play'
  const playColor = ref('white') // human's color in play mode

  function _sync() {
    fen.value = chess.fen()
    history.value = chess.history()
  }

  const turnColor = computed(() => (fen.value && chess.turn() === 'b' ? 'black' : 'white'))

  const gameOver = computed(() => {
    fen.value // reactive dep
    return chess.isGameOver()
  })

  /** Game outcome once over: 'white' | 'black' (winner) | 'draw' | null. */
  const result = computed(() => {
    fen.value // reactive dep
    if (!chess.isGameOver()) return null
    if (chess.isCheckmate()) return chess.turn() === 'w' ? 'black' : 'white'
    return 'draw'
  })

  /** Legal-move map for chessground: from-square → array of destination squares. */
  const legalDests = computed(() => {
    fen.value // reactive dep
    const map = new Map()
    for (const m of chess.moves({ verbose: true })) {
      const arr = map.get(m.from)
      if (arr) arr.push(m.to)
      else map.set(m.from, [m.to])
    }
    return map
  })

  /** Apply a board move {from,to,promotion?}; returns the SAN or null if illegal. */
  function playMove({ from, to, promotion = 'q' }) {
    let move
    try {
      move = chess.move({ from, to, promotion })
    } catch {
      return null
    }
    if (!move) return null
    _sync()
    return move.san
  }

  /** Apply an engine move given in UCI long-algebraic form. */
  function playUci(uci) {
    if (typeof uci !== 'string' || uci.length < 4) return null
    return playMove({
      from: uci.slice(0, 2),
      to: uci.slice(2, 4),
      promotion: uci.length > 4 ? uci[4].toLowerCase() : 'q',
    })
  }

  function reset(startFen = STARTPOS_FEN) {
    chess = new Chess(startFen)
    _sync()
  }

  function undo() {
    chess.undo()
    _sync()
  }

  /** Load an arbitrary position; returns false on an invalid FEN. */
  function setFen(newFen) {
    let next
    try {
      next = new Chess(newFen)
    } catch {
      return false
    }
    chess = next
    _sync()
    return true
  }

  return {
    fen,
    history,
    orientation,
    mode,
    playColor,
    turnColor,
    gameOver,
    result,
    legalDests,
    playMove,
    playUci,
    reset,
    undo,
    setFen,
  }
})
