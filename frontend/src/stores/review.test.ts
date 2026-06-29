import { describe, it, expect, beforeEach, vi } from 'vitest'
import { setActivePinia, createPinia } from 'pinia'

// Mock the API client so the store is tested against fakes, no network.
vi.mock('../api', () => ({
  api: {
    games: {
      analyse: vi.fn(),
    },
  },
}))

import { api } from '../api'
import { useReviewStore } from './review'
import type { GameReview } from '../types'

function sampleReview(): GameReview {
  return {
    start_fen: 'startpos',
    moves: [
      { ply: 1, san: 'e4', eval_cp: 20, classification: 'best', explanation: 'Best move.' },
      {
        ply: 2,
        san: 'd5',
        eval_cp: 80,
        best_move: 'c5',
        played_rank: 4,
        classification: 'mistake',
        explanation: 'Better was c5.',
      },
    ],
    summary: {
      white: { acpl: 10, accuracy: 95.5, inaccuracies: 0, mistakes: 0, blunders: 0 },
      black: { acpl: 60, accuracy: 72.0, inaccuracies: 0, mistakes: 1, blunders: 0 },
    },
  }
}

describe('review store', () => {
  beforeEach(() => {
    setActivePinia(createPinia())
    vi.clearAllMocks()
  })

  it('analyse populates review and the per-ply index', async () => {
    const review = sampleReview()
    vi.mocked(api.games.analyse).mockResolvedValue(review)
    const store = useReviewStore()
    await store.analyse(5)

    expect(api.games.analyse).toHaveBeenCalledWith(5, undefined)
    expect(store.review).toEqual(review)
    expect(store.gameId).toBe(5)
    expect(store.loading).toBe(false)
    expect(store.byPly.get(1)?.san).toBe('e4')
    expect(store.byPly.get(2)?.classification).toBe('mistake')
  })

  it('forwards an explicit depth', async () => {
    vi.mocked(api.games.analyse).mockResolvedValue(sampleReview())
    const store = useReviewStore()
    await store.analyse(5, 16)
    expect(api.games.analyse).toHaveBeenCalledWith(5, 16)
  })

  it('records an error and leaves review null when the api throws', async () => {
    vi.mocked(api.games.analyse).mockRejectedValue(new Error('no engine configured'))
    const store = useReviewStore()
    await store.analyse(5)

    expect(store.error).toBe('no engine configured')
    expect(store.review).toBeNull()
    expect(store.gameId).toBeNull()
    expect(store.loading).toBe(false)
  })

  it('clear resets the store', async () => {
    vi.mocked(api.games.analyse).mockResolvedValue(sampleReview())
    const store = useReviewStore()
    await store.analyse(5)
    store.clear()

    expect(store.review).toBeNull()
    expect(store.gameId).toBeNull()
    expect(store.error).toBeNull()
    expect(store.byPly.size).toBe(0)
  })
})
