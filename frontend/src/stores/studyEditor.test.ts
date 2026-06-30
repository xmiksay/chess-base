import { describe, it, expect, beforeEach, vi } from 'vitest'
import { setActivePinia, createPinia } from 'pinia'

// Mock the API so the editor store is exercised against fakes, no network.
vi.mock('../api', () => ({
  api: {
    studies: {
      get: vi.fn(),
      addMove: vi.fn(),
      annotate: vi.fn(),
      setShapes: vi.fn(),
      promote: vi.fn(),
      reorder: vi.fn(),
      deleteNode: vi.fn(),
    },
  },
}))

import { api } from '../api'
import { useStudiesStore } from './studies'
import { useStudyEditorStore } from './studyEditor'
import type { Study } from '../types'

// 1.e4 e5 (1...c5) — mainline plus one variation; ids are dense.
function sampleStudy(): Study {
  return {
    id: 10,
    database_id: 1,
    name: 'Open Games',
    global: false,
    owner_id: null,
    tree: {
      root: 0,
      nodes: [
        { id: 0, parent: null, san: null, comment: null, nags: [], children: [1] },
        { id: 1, parent: 0, san: 'e4', comment: null, nags: [], children: [2, 3] },
        { id: 2, parent: 1, san: 'e5', comment: null, nags: [], children: [] },
        { id: 3, parent: 1, san: 'c5', comment: null, nags: [], children: [] },
      ],
    },
  }
}

describe('studyEditor store', () => {
  let studies: ReturnType<typeof useStudiesStore>
  let editor: ReturnType<typeof useStudyEditorStore>

  beforeEach(() => {
    setActivePinia(createPinia())
    vi.clearAllMocks()
    studies = useStudiesStore()
    editor = useStudyEditorStore()
    studies.current = sampleStudy()
    editor.select(0)
  })

  it('derives the board position from the selected node', () => {
    editor.select(2)
    expect(editor.line).toEqual(['e4', 'e5'])
    expect(editor.fen.startsWith('rnbqkbnr/pppp1ppp')).toBe(true)
    expect(editor.lastMove).toEqual(['e7', 'e5'])
    expect(editor.turnColor).toBe('white')
  })

  it('navigates the tree with select / forward / back / start / end', () => {
    editor.forward()
    expect(editor.nodeId).toBe(1) // mainline child of root
    editor.goToEnd()
    expect(editor.nodeId).toBe(2) // follows children[0] to the leaf
    editor.back()
    expect(editor.nodeId).toBe(1)
    editor.goToStart()
    expect(editor.nodeId).toBe(0)
    expect(editor.atStart).toBe(true)
  })

  it('addSan reuses an existing child without hitting the API', async () => {
    editor.select(1)
    const id = await editor.addSan('c5')
    expect(id).toBe(3)
    expect(editor.nodeId).toBe(3)
    expect(api.studies.addMove).not.toHaveBeenCalled()
  })

  it('addSan appends a new move via the API and selects it', async () => {
    const grown = sampleStudy()
    grown.tree.nodes.push({ id: 4, parent: 0, san: 'd4', comment: null, nags: [], children: [] })
    grown.tree.nodes[0].children.push(4)
    vi.mocked(api.studies.addMove).mockResolvedValue({ new_node_id: 4, study: grown })

    const id = await editor.addSan('d4')
    expect(api.studies.addMove).toHaveBeenCalledWith(10, 0, 'd4')
    expect(id).toBe(4)
    expect(editor.nodeId).toBe(4)
    expect(studies.current!.tree.nodes).toHaveLength(5)
  })

  it('playMove turns a board drag into a SAN append/navigation', async () => {
    // e2e4 from the start matches the existing mainline child, no API call.
    const id = await editor.playMove({ from: 'e2', to: 'e4' })
    expect(id).toBe(1)
    expect(editor.nodeId).toBe(1)
    expect(api.studies.addMove).not.toHaveBeenCalled()
  })

  it('playMove returns null for an illegal drag', async () => {
    const id = await editor.playMove({ from: 'e2', to: 'e5' })
    expect(id).toBeNull()
  })

  it('annotate sends comment/NAG and stores the refreshed study', async () => {
    const annotated = sampleStudy()
    annotated.tree.nodes[1].comment = 'King pawn'
    vi.mocked(api.studies.annotate).mockResolvedValue(annotated)

    await editor.annotate({ comment: 'King pawn' }, 1)
    expect(api.studies.annotate).toHaveBeenCalledWith(10, 1, { comment: 'King pawn' })
    expect(studies.current!.tree.nodes[1].comment).toBe('King pawn')
  })

  it('setShapes pins a plan to the selected node and stores the refreshed study', async () => {
    const pinned = sampleStudy()
    const shapes = [{ orig: 'g1', dest: 'f3', brush: 'green' }]
    pinned.tree.nodes[1].shapes = shapes
    vi.mocked(api.studies.setShapes).mockResolvedValue(pinned)

    editor.select(1)
    await editor.setShapes(shapes)
    expect(api.studies.setShapes).toHaveBeenCalledWith(10, 1, shapes)
    expect(studies.current!.tree.nodes[1].shapes).toEqual(shapes)
  })

  it('seeds the board from a set-up start_fen and replays edits from it', () => {
    // The Catalan after 1.d4 Nf6 2.c4 e6 3.g3, Black to move — `d5` is illegal
    // from the standard start, so the board must seed from start_fen.
    const fen = 'rnbqkb1r/pppp1ppp/4pn2/8/2PP4/6P1/PP2PP1P/RNBQKBNR b KQkq - 0 3'
    studies.current = {
      id: 11,
      database_id: 1,
      name: 'Catalan',
      global: false,
      owner_id: null,
      tree: {
        root: 0,
        start_fen: fen,
        nodes: [
          { id: 0, parent: null, san: null, comment: null, nags: [], children: [1] },
          { id: 1, parent: 0, san: 'd5', comment: null, nags: [], children: [] },
        ],
      },
    }
    editor.select(0)
    expect(editor.fen.startsWith('rnbqkb1r/pppp1ppp/4pn2/8/2PP4/6P1/PP2PP1P/RNBQKBNR b')).toBe(
      true,
    )
    expect(editor.turnColor).toBe('black')

    editor.select(1)
    expect(editor.line).toEqual(['d5'])
    expect(editor.fen.startsWith('rnbqkb1r/ppp2ppp/4pn2/3p4/2PP4/6P1/PP2PP1P/RNBQKBNR w')).toBe(
      true,
    )
  })

  it('deleteNode refreshes the tree and resets the selection to the root', async () => {
    const pruned = sampleStudy()
    pruned.tree.nodes[1].children = [2] // variation removed
    pruned.tree.nodes = pruned.tree.nodes.filter((n) => n.id !== 3)
    vi.mocked(api.studies.deleteNode).mockResolvedValue(pruned)

    editor.select(3)
    await editor.deleteNode(3)
    expect(api.studies.deleteNode).toHaveBeenCalledWith(10, 3)
    expect(editor.nodeId).toBe(0)
    expect(studies.current!.tree.nodes).toHaveLength(3)
  })
})
