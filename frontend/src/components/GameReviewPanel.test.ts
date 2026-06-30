import { describe, it, expect, beforeEach, vi } from 'vitest'
import { setActivePinia, createPinia } from 'pinia'
import { mount } from '@vue/test-utils'

// Mock the API client: the panel drives the review (analyse) and export actions.
vi.mock('../api', () => ({
  api: {
    games: {
      analyse: vi.fn(),
      exportPgn: vi.fn(),
      get: vi.fn(),
      tree: vi.fn(),
      list: vi.fn(),
    },
  },
}))

import { api } from '../api'
import GameReviewPanel from './GameReviewPanel.vue'
import EvalGraph from './EvalGraph.vue'
import { useGamesStore } from '../stores/games'
import { useReviewStore } from '../stores/review'
import { appendChild, emptyTree } from '../lib/moveTree'
import { STARTPOS_FEN } from '../lib/fen'
import type { GameReview, MoveTree } from '../types'

function lineTree(sans: string[]): MoveTree {
  let tree = emptyTree()
  let parent = tree.root
  for (const san of sans) {
    const r = appendChild(tree, parent, san)!
    tree = r.tree
    parent = r.id
  }
  return tree
}

function sampleReview(): GameReview {
  return {
    start_fen: 'startpos',
    moves: [
      { ply: 1, san: 'e4', eval_cp: 20, classification: 'best', explanation: 'Best.' },
      { ply: 2, san: 'd5', eval_cp: 80, classification: 'mistake', explanation: 'c5 better.' },
    ],
    summary: {
      white: { acpl: 5, accuracy: 98, inaccuracies: 0, mistakes: 0, blunders: 0 },
      black: { acpl: 40, accuracy: 80, inaccuracies: 0, mistakes: 1, blunders: 0 },
    },
  }
}

describe('GameReviewPanel — EvalGraph wiring', () => {
  beforeEach(() => {
    setActivePinia(createPinia())
    vi.clearAllMocks()
  })

  async function setup() {
    const games = useGamesStore()
    games.load(lineTree(['e4', 'd5']), STARTPOS_FEN) // nodes 0(root) 1.e4 2.d5
    const review = useReviewStore()
    vi.mocked(api.games.analyse).mockResolvedValue(sampleReview())
    await review.analyse(5)
    const wrapper = mount(GameReviewPanel, { props: { engineEnabled: true } })
    return { games, review, wrapper }
  }

  it('feeds EvalGraph the current cursor mapped through plyOf', async () => {
    const { games, wrapper } = await setup()
    games.goto(games.nodeAtPly(2)!) // cursor at mainline ply 2
    await wrapper.vm.$nextTick()

    const graph = wrapper.findComponent(EvalGraph)
    expect(graph.props('currentPly')).toBe(2)
  })

  it('selecting a graph point navigates the board via nodeAtPly', async () => {
    const { games, wrapper } = await setup()
    const graph = wrapper.findComponent(EvalGraph)
    // The first plotted point is ply 1 → its mainline node.
    await graph.findAll('[data-test="eval-point"]')[0].trigger('click')

    expect(games.currentId).toBe(games.nodeAtPly(1))
  })
})
