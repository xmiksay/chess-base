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

  describe('tree cursor', () => {
    function withMoves() {
      const game = useGameStore()
      game.playMove({ from: 'e2', to: 'e4' })
      game.playMove({ from: 'e7', to: 'e5' })
      game.playMove({ from: 'g1', to: 'f3' })
      return game
    }

    /** Node id reached after replaying `sans` from the start (via first()/next-ish). */
    function nodeAfter(game: ReturnType<typeof useGameStore>, sans: string[]): number {
      game.first()
      for (const san of sans) {
        const before = game.currentId
        game.next() // walk the mainline one step
        expect(game.history.at(-1)).toBe(san)
        expect(game.currentId).not.toBe(before)
      }
      return game.currentId
    }

    it('advances the cursor to the tip as moves are played', () => {
      const game = withMoves()
      expect(game.history).toEqual(['e4', 'e5', 'Nf3'])
      expect(game.atEnd).toBe(true)
      expect(game.atStart).toBe(false)
    })

    it('prev/next step the cursor and follow the fen', () => {
      const game = withMoves()
      const tip = game.fen
      game.prev()
      expect(game.history).toEqual(['e4', 'e5'])
      expect(game.fen).not.toBe(tip)
      expect(game.turnColor).toBe('white') // after 1.e4 e5, White to move
      game.next()
      expect(game.history).toEqual(['e4', 'e5', 'Nf3'])
      expect(game.fen).toBe(tip)
    })

    it('first/last jump to the bounds; goto ignores unknown ids', () => {
      const game = withMoves()
      const tip = game.currentId
      game.first()
      expect(game.atStart).toBe(true)
      expect(game.fen).toBe(STARTPOS_FEN)
      game.goto(99999) // no such node — stays put
      expect(game.atStart).toBe(true)
      game.last()
      expect(game.currentId).toBe(tip)
      expect(game.atEnd).toBe(true)
    })

    it('reports the fen and last move at the selected node', () => {
      const game = withMoves()
      const afterE4 = nodeAfter(game, ['e4'])
      game.goto(afterE4)
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

    it('playing a move off the line branches a variation instead of truncating', () => {
      const game = withMoves()
      const afterE4 = nodeAfter(game, ['e4'])
      game.goto(afterE4)
      game.playMove({ from: 'c7', to: 'c5' }) // Sicilian alongside 1...e5
      expect(game.history).toEqual(['e4', 'c5'])
      expect(game.atEnd).toBe(true)
      // The original 1...e5 mainline is preserved as a sibling, not overwritten.
      const e4Node = game.tree.nodes.find((n) => n.id === afterE4)
      expect(e4Node?.children).toHaveLength(2)
      game.goto(afterE4)
      game.next() // children[0] is still the mainline e5
      expect(game.history).toEqual(['e4', 'e5'])
    })

    it('replaying an existing move follows it rather than duplicating', () => {
      const game = withMoves()
      const afterE4 = nodeAfter(game, ['e4'])
      const before = game.tree.nodes.length
      game.goto(afterE4)
      game.playMove({ from: 'e7', to: 'e5' }) // the move already on the line
      expect(game.history).toEqual(['e4', 'e5'])
      expect(game.tree.nodes).toHaveLength(before) // no new node created
    })

    it('undo deletes the current node and follows the cursor to its parent', () => {
      const game = withMoves()
      game.undo()
      expect(game.history).toEqual(['e4', 'e5'])
      expect(game.atEnd).toBe(true)
      expect(game.tree.nodes).toHaveLength(3) // root + e4 + e5
    })
  })
})
