// Pinia store for the variation-tree editor (issue #8). Mirrors the open study's
// backend `MoveTree`, tracks the selected node, derives the board position for
// that node (via chess.js), and turns board moves into tree edits — navigating
// to an existing child or appending a new move/variation through `api.studies`.
// Lifecycle (open/create/import) stays in the studies store, which owns `current`.

import { defineStore } from 'pinia'
import { ref, computed } from 'vue'
import { Chess } from 'chess.js'
import { api } from '../api'
import { useStudiesStore } from './studies'
import { childWithSan, firstChild, getNode, lastMainlineId, sanPath } from '../lib/moveTree'
import type { Annotation, BoardMove, Shape, Square } from '../types'

/** A chess.js seeded from a study's set-up `start_fen`, or the standard start
 *  when absent or malformed (a bad origin must not blank the board). */
function startChess(startFen?: string): Chess {
  if (startFen) {
    try {
      return new Chess(startFen)
    } catch {
      // fall through to the standard start position
    }
  }
  return new Chess()
}

export const useStudyEditorStore = defineStore('studyEditor', () => {
  const studies = useStudiesStore()

  const nodeId = ref(0) // selected node id; the tree root means "start position"

  const tree = computed(() => studies.current?.tree ?? null)
  const studyId = computed(() => studies.current?.id ?? null)
  const currentNode = computed(() => (tree.value ? getNode(tree.value, nodeId.value) : null))

  // SAN line from the root to the selected node, replayed into a chess.js
  // position so the board, legal moves and last-move highlight follow selection.
  const line = computed(() => (tree.value ? sanPath(tree.value, nodeId.value) : []))
  const _chess = computed(() => {
    const c = startChess(tree.value?.start_fen)
    for (const san of line.value) {
      try {
        c.move(san)
      } catch {
        break // a malformed stored line shouldn't blank the board
      }
    }
    return c
  })

  const fen = computed(() => _chess.value.fen())
  const turnColor = computed(() => (_chess.value.turn() === 'b' ? 'black' : 'white'))
  const lastMove = computed<[Square, Square] | null>(() => {
    const h = _chess.value.history({ verbose: true })
    if (!h.length) return null
    const m = h[h.length - 1]
    return [m.from, m.to]
  })
  const legalDests = computed(() => {
    const map = new Map<string, string[]>()
    for (const m of _chess.value.moves({ verbose: true })) {
      const arr = map.get(m.from)
      if (arr) arr.push(m.to)
      else map.set(m.from, [m.to])
    }
    return map
  })

  const atStart = computed(() => !currentNode.value || currentNode.value.parent == null)
  const atEnd = computed(() => firstChild(tree.value ?? { root: 0, nodes: [] }, nodeId.value) == null)

  /** Open a study (loads its tree) and select the start position. */
  async function open(id: number) {
    await studies.open(id)
    nodeId.value = tree.value?.root ?? 0
  }

  // --- navigation -----------------------------------------------------------
  function select(id: number) {
    nodeId.value = id
  }
  function goToStart() {
    nodeId.value = tree.value?.root ?? 0
  }
  function back() {
    const parent = currentNode.value?.parent
    if (parent != null) nodeId.value = parent
  }
  function forward() {
    const next = firstChild(tree.value!, nodeId.value)
    if (next != null) nodeId.value = next
  }
  function goToEnd() {
    if (tree.value) nodeId.value = lastMainlineId(tree.value, nodeId.value)
  }

  // --- editing --------------------------------------------------------------

  /** Append `san` under the selected node; reuse the child if it already exists. */
  async function addSan(san: string) {
    const existing = childWithSan(tree.value!, nodeId.value, san)
    if (existing != null) {
      nodeId.value = existing
      return existing
    }
    const { new_node_id, study } = await api.studies.addMove(studyId.value!, nodeId.value, san)
    studies.current = study
    nodeId.value = new_node_id
    return new_node_id
  }

  /**
   * Apply a board drag {from,to,promotion?}: derive the SAN against the current
   * position, then append it (or navigate to the matching child). Returns the
   * resulting node id, or null when the move is illegal.
   */
  async function playMove({ from, to, promotion = 'q' }: BoardMove): Promise<number | null> {
    let move
    try {
      move = new Chess(fen.value).move({ from, to, promotion })
    } catch {
      return null
    }
    if (!move) return null
    return addSan(move.san)
  }

  /** Set the comment and/or NAG on a node (defaults to the selected one). */
  async function annotate({ comment, nag }: Annotation = {}, id: number = nodeId.value) {
    studies.current = await api.studies.annotate(studyId.value!, id, { comment, nag })
  }

  /** Pin board shapes (a plan) to a node, or clear them with `[]` (#61). */
  async function setShapes(shapes: Shape[], id: number = nodeId.value) {
    studies.current = await api.studies.setShapes(studyId.value!, id, shapes)
  }

  /**
   * Fill `[%eval]` on every non-terminal node via the engine (#162), so the
   * exported PGN carries evals Lichess renders. Eval-only — comments / NAGs /
   * shapes are left untouched. Returns the refreshed study into `current`.
   */
  async function analyseStudy() {
    studies.current = await api.studies.analyse(studyId.value!)
  }

  /** Promote a variation toward the mainline. */
  async function promote(id: number) {
    studies.current = await api.studies.promote(studyId.value!, id)
  }

  /** Reorder a node among its siblings (0 = mainline). */
  async function reorder(id: number, index: number) {
    studies.current = await api.studies.reorder(studyId.value!, id, index)
  }

  /**
   * Delete a node and its subtree. The backend re-indexes node ids, so the
   * returned tree is authoritative; we drop the selection back to the start.
   */
  async function deleteNode(id: number) {
    studies.current = await api.studies.deleteNode(studyId.value!, id)
    nodeId.value = tree.value?.root ?? 0
  }

  return {
    nodeId,
    tree,
    studyId,
    currentNode,
    line,
    fen,
    turnColor,
    lastMove,
    legalDests,
    atStart,
    atEnd,
    open,
    select,
    goToStart,
    back,
    forward,
    goToEnd,
    addSan,
    playMove,
    annotate,
    setShapes,
    analyseStudy,
    promote,
    reorder,
    deleteNode,
  }
})
