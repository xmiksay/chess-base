// Snapshot-based undo for the analysis board.
//
// Snapshots, not inverse operations: a `MoveTree` is immutable (every mutator
// returns a new object) and small enough that keeping whole trees is cheaper in
// complexity than maintaining an inverse for each op. Deleting a subtree is the
// case that settles it — its inverse has to restore the node, its entire
// subtree, and its exact sibling position, which is precisely the snapshot.
//
// Framework-free: the store wires it to Ctrl+Z, the tests drive it directly.

/** One restorable point in time. `T` is whatever the caller needs to restore. */
export interface UndoEntry<T> {
  /** Short human label, e.g. "delete 4...Nf6" — surfaced in the UI tooltip. */
  label: string
  state: T
}

export interface UndoStack<T> {
  /** Record the state *before* a mutation. Clears the redo branch. */
  push(label: string, state: T): void
  /** Step back, handing `current` in so it can be redone. Null when empty. */
  undo(current: T): UndoEntry<T> | null
  /** Step forward again. Null when there is nothing to redo. */
  redo(current: T): UndoEntry<T> | null
  canUndo(): boolean
  canRedo(): boolean
  /** Forget everything — used after a commit, which cannot be undone. */
  clear(): void
  /** Label of the next undo, for a "Undo delete 4...Nf6" tooltip. */
  peek(): string | null
}

/**
 * A bounded undo stack. `limit` caps memory: the oldest entry is dropped once
 * it is exceeded, so a long session cannot pin every tree it ever held.
 */
export function createUndoStack<T>(limit = 50): UndoStack<T> {
  let past: Array<UndoEntry<T>> = []
  let future: Array<UndoEntry<T>> = []

  return {
    push(label, state) {
      past.push({ label, state })
      if (past.length > limit) past = past.slice(past.length - limit)
      // A new edit invalidates anything that was undone — the usual branch rule.
      future = []
    },

    undo(current) {
      const entry = past.pop()
      if (!entry) return null
      future.push({ label: entry.label, state: current })
      return entry
    },

    redo(current) {
      const entry = future.pop()
      if (!entry) return null
      past.push({ label: entry.label, state: current })
      return entry
    },

    canUndo: () => past.length > 0,
    canRedo: () => future.length > 0,

    clear() {
      past = []
      future = []
    },

    peek: () => past[past.length - 1]?.label ?? null,
  }
}
