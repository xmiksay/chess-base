import { describe, it, expect, beforeEach } from 'vitest'
import { setActivePinia, createPinia } from 'pinia'
import { useGameStore } from './game'
import { STARTPOS_FEN } from '../lib/fen'

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

  describe('ply cursor', () => {
    function withMoves() {
      const game = useGameStore()
      game.playMove({ from: 'e2', to: 'e4' })
      game.playMove({ from: 'e7', to: 'e5' })
      game.playMove({ from: 'g1', to: 'f3' })
      return game
    }

    it('advances the cursor to the tip as moves are played', () => {
      const game = withMoves()
      expect(game.ply).toBe(3)
      expect(game.atEnd).toBe(true)
      expect(game.atStart).toBe(false)
    })

    it('prev/next step the cursor and follow the fen', () => {
      const game = withMoves()
      const tip = game.fen
      game.prev()
      expect(game.ply).toBe(2)
      expect(game.fen).not.toBe(tip)
      expect(game.turnColor).toBe('white') // after 1.e4 e5, White to move
      game.next()
      expect(game.ply).toBe(3)
      expect(game.fen).toBe(tip)
    })

    it('first/last jump to the bounds and clamp goto', () => {
      const game = withMoves()
      game.first()
      expect(game.ply).toBe(0)
      expect(game.atStart).toBe(true)
      expect(game.fen).toBe(STARTPOS_FEN)
      game.goto(-5)
      expect(game.ply).toBe(0)
      game.goto(99)
      expect(game.ply).toBe(3)
      game.last()
      expect(game.ply).toBe(3)
    })

    it('reports the fen and last move at the selected ply', () => {
      const game = withMoves()
      game.goto(1) // after 1.e4
      expect(game.fen.startsWith('rnbqkbnr/pppppppp/8/8/4P3/8/PPPP1PPP/RNBQKBNR')).toBe(true)
      expect(game.turnColor).toBe('black')
      expect(game.lastMove).toEqual(['e2', 'e4'])
      game.first()
      expect(game.lastMove).toBeNull()
    })

    it('legal moves track the cursor position', () => {
      const game = withMoves()
      game.first()
      expect(game.legalDests.get('e2')).toContain('e4')
      expect(game.legalDests.get('g1')).toContain('f3')
    })

    it('playing a move at a non-tip ply truncates the future line', () => {
      const game = withMoves()
      game.goto(1) // back to after 1.e4
      game.playMove({ from: 'c7', to: 'c5' }) // Sicilian instead of 1...e5
      expect(game.history).toEqual(['e4', 'c5'])
      expect(game.ply).toBe(2)
      expect(game.atEnd).toBe(true)
    })

    it('undo removes the last move and follows the cursor back', () => {
      const game = withMoves()
      game.undo()
      expect(game.history).toEqual(['e4', 'e5'])
      expect(game.ply).toBe(2)
      expect(game.atEnd).toBe(true)
    })
  })
})
