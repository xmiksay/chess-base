import { describe, it, expect } from 'vitest'
import { createUndoStack } from './undoStack'

describe('undoStack', () => {
  it('steps back through snapshots in reverse order', () => {
    const s = createUndoStack<string>()
    s.push('a', 'A')
    s.push('b', 'B')

    expect(s.undo('C')!.state).toBe('B')
    expect(s.undo('B')!.state).toBe('A')
    expect(s.undo('A')).toBeNull()
  })

  it('redoes what it undid', () => {
    const s = createUndoStack<string>()
    s.push('a', 'A')

    const undone = s.undo('B')!
    expect(undone.state).toBe('A')
    expect(s.canRedo()).toBe(true)
    expect(s.redo('A')!.state).toBe('B')
    expect(s.canRedo()).toBe(false)
  })

  it('a new edit discards the redo branch', () => {
    const s = createUndoStack<string>()
    s.push('a', 'A')
    s.undo('B')
    expect(s.canRedo()).toBe(true)

    s.push('c', 'A')
    expect(s.canRedo()).toBe(false)
  })

  it('drops the oldest entry past the limit', () => {
    const s = createUndoStack<number>(2)
    s.push('1', 1)
    s.push('2', 2)
    s.push('3', 3)

    expect(s.undo(4)!.state).toBe(3)
    expect(s.undo(3)!.state).toBe(2)
    expect(s.undo(2)).toBeNull() // 1 was evicted
  })

  it('clear forgets both directions — a commit cannot be undone', () => {
    const s = createUndoStack<string>()
    s.push('a', 'A')
    s.undo('B')
    s.clear()

    expect(s.canUndo()).toBe(false)
    expect(s.canRedo()).toBe(false)
    expect(s.peek()).toBeNull()
  })

  it('names the next undo for the UI', () => {
    const s = createUndoStack<string>()
    expect(s.peek()).toBeNull()
    s.push('delete 4...Nf6', 'A')
    expect(s.peek()).toBe('delete 4...Nf6')
  })
})
