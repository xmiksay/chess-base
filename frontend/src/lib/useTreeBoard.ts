// Shared board state machine (issue #134): the client-side variation-tree +
// cursor that every board page drives. Lifted out of `stores/game.ts` so the
// Analyse, Study and Game-review boards share one implementation.
//
// Moves are kept as a `MoveTree` (the same shape studies use, see src/pgn_tree.rs)
// with a `currentId` cursor backed by a chess.js instance positioned at the
// cursor for legality. Navigation moves the cursor without mutating the tree;
// playing a move that already exists as a continuation just follows it, while a
// new move branches off as a variation rather than truncating the line.

import { ref, computed } from 'vue'
import { Chess } from 'chess.js'
import { STARTPOS_FEN } from './fen'
import {
  emptyTree,
  appendChild,
  deleteSubtree,
  childWithSan,
  firstChild,
  getNode,
  lastMainlineId,
  reorderChild,
  sanPath,
  siblingIndex,
} from './moveTree'
import type { BoardMove, Color, Dests, MoveTree, Square } from '../types'

export function useTreeBoard() {
  let chess: Chess = new Chess() // positioned at the current node

  const startFen = ref(chess.fen()) // position at the root (before the first move)
  const tree = ref<MoveTree>(emptyTree())
  const currentId = ref(tree.value.root) // cursor: which node the board shows
  const fen = ref(chess.fen())
  const lastMove = ref<[Square, Square] | null>(null) // move into the current node
  const orientation = ref<Color>('white') // board orientation

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

  /** Delete any node and its subtree, dropping the cursor to its parent. */
  function removeNode(id: number) {
    if (id === tree.value.root) return
    const { tree: pruned, parentId } = deleteSubtree(tree.value, id)
    tree.value = pruned
    seek(currentId.value === id || !getNode(pruned, currentId.value) ? parentId : currentId.value)
  }

  /** Promote a node to its parent's mainline continuation. */
  function promoteNode(id: number) {
    tree.value = reorderChild(tree.value, id, 0)
  }

  /** Demote a node one step away from the mainline (its sibling index + 1). */
  function demoteNode(id: number) {
    const idx = siblingIndex(tree.value, id)
    if (idx < 0) return
    tree.value = reorderChild(tree.value, id, idx + 1)
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

  /**
   * Seed an existing tree (e.g. a game's parsed mainline + variations), starting
   * from `start` and positioning the cursor at the root. Used by the games store
   * to drop a fetched game onto the shared board.
   */
  function load(newTree: MoveTree, start: string = STARTPOS_FEN): boolean {
    let next
    try {
      next = new Chess(start)
    } catch {
      return false
    }
    chess = next
    startFen.value = chess.fen()
    tree.value = newTree
    seek(newTree.root)
    return true
  }

  return {
    startFen,
    tree,
    currentId,
    fen,
    lastMove,
    orientation,
    history,
    atStart,
    atEnd,
    turnColor,
    gameOver,
    result,
    legalDests,
    seek,
    playMove,
    playUci,
    goto,
    next,
    prev,
    first,
    last,
    reset,
    undo,
    removeNode,
    promoteNode,
    demoteNode,
    setFen,
    load,
  }
}
