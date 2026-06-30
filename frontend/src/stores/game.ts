// Pinia store holding the board/game state, backed by a chess.js instance for
// legality. Drives both the analysis board (free movement) and play-vs-engine.
//
// Moves are kept as a `MoveTree` (the same shape studies use, see src/pgn_tree.rs)
// with a `currentId` cursor. Navigation moves the cursor without mutating the
// tree; playing a move that already exists as a continuation just follows it,
// while a new move branches off as a variation rather than truncating the line.

import { defineStore } from 'pinia'
import { ref, computed } from 'vue'
import { Chess } from 'chess.js'
import { STARTPOS_FEN } from '../lib/fen'
import {
  emptyTree,
  appendChild,
  deleteSubtree,
  childWithSan,
  firstChild,
  getNode,
  lastMainlineId,
  sanPath,
} from '../lib/moveTree'
import type { BoardMove, Color, Dests, MoveTree, Square } from '../types'

export const useGameStore = defineStore('game', () => {
  let chess: Chess = new Chess() // positioned at the current node

  const startFen = ref(chess.fen()) // position at the root (before the first move)
  const tree = ref<MoveTree>(emptyTree())
  const currentId = ref(tree.value.root) // cursor: which node the board shows
  const fen = ref(chess.fen())
  const lastMove = ref<[Square, Square] | null>(null) // move into the current node
  const orientation = ref<Color>('white') // board orientation
  const mode = ref<'analyse' | 'play'>('analyse') // 'analyse' | 'play'
  const playColor = ref<Color>('white') // human's color in play mode

  /** SAN moves from the root to the current node — drives the move panel / export. */
  const history = computed<string[]>(() => sanPath(tree.value, currentId.value))

  const atStart = computed(() => currentId.value === tree.value.root)
  const atEnd = computed(() => firstChild(tree.value, currentId.value) == null)

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

  /** Reposition `chess` at node `id` by replaying the line, syncing fen/lastMove. */
  function seek(id: number) {
    chess = new Chess(startFen.value)
    for (const san of sanPath(tree.value, id)) chess.move(san)
    currentId.value = id
    fen.value = chess.fen()
    const hist = chess.history({ verbose: true })
    const last = hist[hist.length - 1]
    lastMove.value = last ? [last.from as Square, last.to as Square] : null
  }

  /** Apply a board move {from,to,promotion?}; returns the SAN or null if illegal. */
  function playMove({ from, to, promotion = 'q' }: BoardMove): string | null {
    let move
    try {
      move = chess.move({ from, to, promotion })
    } catch {
      return null
    }
    if (!move) return null
    // Follow an existing continuation, else branch a new variation off the cursor.
    const existing = childWithSan(tree.value, currentId.value, move.san)
    if (existing != null) {
      currentId.value = existing
    } else {
      const appended = appendChild(tree.value, currentId.value, move.san)
      if (appended) {
        tree.value = appended.tree
        currentId.value = appended.id
      }
    }
    fen.value = chess.fen()
    lastMove.value = [move.from as Square, move.to as Square]
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

  /** Move the cursor to node `id` (ignored if absent); the board/engine follow `fen`. */
  function goto(id: number) {
    if (id === currentId.value || !getNode(tree.value, id)) return
    seek(id)
  }

  const next = () => {
    const child = firstChild(tree.value, currentId.value)
    if (child != null) seek(child)
  }
  const prev = () => {
    const parent = getNode(tree.value, currentId.value)?.parent
    if (parent != null) seek(parent)
  }
  const first = () => seek(tree.value.root)
  const last = () => seek(lastMainlineId(tree.value, currentId.value))

  function reset(fenString: string = STARTPOS_FEN) {
    chess = new Chess(fenString)
    startFen.value = chess.fen()
    tree.value = emptyTree()
    currentId.value = tree.value.root
    fen.value = chess.fen()
    lastMove.value = null
  }

  /** Delete the current node and its subtree, dropping the cursor to its parent. */
  function undo() {
    if (currentId.value === tree.value.root) return
    const { tree: pruned, parentId } = deleteSubtree(tree.value, currentId.value)
    tree.value = pruned
    seek(parentId)
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
    tree.value = emptyTree()
    currentId.value = tree.value.root
    fen.value = chess.fen()
    lastMove.value = null
    return true
  }

  return {
    fen,
    history,
    tree,
    currentId,
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
