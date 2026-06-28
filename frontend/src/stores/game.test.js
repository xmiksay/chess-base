import { describe, it, expect, beforeEach } from 'vitest'
import { setActivePinia, createPinia } from 'pinia'
import { useGameStore } from './game.js'
import { STARTPOS_FEN } from '../lib/fen.js'

describe('game store', () => {
  beforeEach(() => setActivePinia(createPinia()))

  it('starts at the initial position with White to move', () => {
    const game = useGameStore()
    expect(game.fen).toBe(STARTPOS_FEN)
    expect(game.turnColor).toBe('white')
    expect(game.gameOver).toBe(false)
  })

  it('applies a legal move and records SAN history', () => {
    const game = useGameStore()
    expect(game.playMove({ from: 'e2', to: 'e4' })).toBe('e4')
    expect(game.turnColor).toBe('black')
    expect(game.history).toEqual(['e4'])
  })

  it('rejects an illegal move', () => {
    const game = useGameStore()
    expect(game.playMove({ from: 'e2', to: 'e5' })).toBeNull()
    expect(game.history).toEqual([])
  })

  it('applies an engine move given in UCI', () => {
    const game = useGameStore()
    expect(game.playUci('g1f3')).toBe('Nf3')
    expect(game.turnColor).toBe('black')
  })

  it('exposes legal destinations for chessground', () => {
    const game = useGameStore()
    const dests = game.legalDests
    expect(dests.get('e2')).toContain('e4')
    expect(dests.get('g1')).toContain('f3')
  })

  it('detects checkmate and reports the winner', () => {
    const game = useGameStore()
    // Fool's mate.
    game.playMove({ from: 'f2', to: 'f3' })
    game.playMove({ from: 'e7', to: 'e5' })
    game.playMove({ from: 'g2', to: 'g4' })
    game.playMove({ from: 'd8', to: 'h4' })
    expect(game.gameOver).toBe(true)
    expect(game.result).toBe('black')
  })

  it('resets to the start position', () => {
    const game = useGameStore()
    game.playMove({ from: 'e2', to: 'e4' })
    game.reset()
    expect(game.fen).toBe(STARTPOS_FEN)
    expect(game.history).toEqual([])
  })

  it('loads a FEN and rejects an invalid one', () => {
    const game = useGameStore()
    expect(game.setFen('rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR b KQkq - 0 1')).toBe(true)
    expect(game.turnColor).toBe('black')
    expect(game.setFen('not-a-fen')).toBe(false)
  })
})
