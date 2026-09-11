# ADR-0051 — Notation panel: flat rows, depth signals, context-menu actions

Status: accepted

> 0047–0050 are reserved for the in-flight round-trip, Lichess-explorer,
> analysis-surface and scratch-editing decisions of the same effort; this one
> landed first and takes the next free number.

## Context

The move list was the single biggest usability failure in the app. Four concrete
defects, all confirmed in code:

1. **Accidental deletion.** `MoveTreeLine.vue` rendered the `⤴ ⤵ ✕` toolbar
   *inline in the flowing move text*, immediately after the selected move.
   Selecting a move therefore reflowed every following move to the right, under
   the cursor — so the next click landed on the previous move's `✕`, a ~12px
   `px-1 text-xs` target. A `window.confirm` guarded it, but the interaction was
   broken by construction. There was no undo anywhere in the app.
2. **Depth was invisible.** The renderer styled depth 0 one way and *everything
   else* another, so a depth-1 and a depth-5 variation were identical; only a
   1px `border-l` and 2 spaces of indent distinguished them.
3. **Comments were hidden.** A commented move showed a green `•`; the text lived
   in a side box that only ever showed the selected node. Reading a game's
   annotations meant clicking every move.
4. **Nothing scrolled into view.** `grep -rn scrollIntoView` over the whole
   frontend returned zero hits, and GamesView's list had no height cap at all,
   so a long game pushed the board and engine panel off-screen.

Plus an O(n²): `moveTree.ts` was written against `nodes.find(…)`, and
`MoveTree.vue` re-ran `treeTokens` over the entire tree on every ply change.

## Decision

**Rows, not a nested token tree.** `lib/moveTreeRows.treeRows` flattens the tree
into a `MoveRow[]` — one row per line, carrying its own `depth`. The panel keys
a `v-for` on rows and each row carries a `v-memo`, so stepping the cursor
re-renders exactly the two rows that changed. A row knowing its depth as a
*number* is also what makes signal (2) expressible at all.

**Three depth signals, not one.** Indent (a runtime `paddingLeft`), a colour
rail from new `--depth-1..5` tokens (a literal class map, so Tailwind's scanner
sees the names), and a type/weight ladder. Clamped at 5; deeper reuses level 5.

**Comments inline.** `inlineComments` defaults true; the dot marker remains
available behind the prop rather than being deleted, so the behaviour change is
an explicit choice at each call site.

**Node actions are a context menu.** Right-click opens `NodeContextMenu`,
positioned *over* the text and clamped to the viewport. A menu cannot reflow the
move flow — that is the structural fix, not a bigger hit target. Delete
additionally arms after 250ms so a menu opening under a moving cursor cannot eat
a stray click. A test asserts the move flow contains only move buttons, at every
depth, so this cannot regress.

**Indexing.** `lib/moveTreeIndex.indexOf` builds `byId` / `mainline` / `plyOf` /
`depthOf` in one pass, memoized in a `WeakMap` keyed on the tree object. This is
safe *because* every mutator in `moveTree.ts` returns a new tree rather than
mutating in place, so a cached index cannot outlive what it describes.
`getNode`, `nodeMap` and `mainlinePath` delegate to it with unchanged
signatures — which is why all 24 existing `moveTree` tests pass untouched, and
that is the evidence the refactor preserves behaviour.

**Scroll math is pure.** `lib/scrollSync.scrollOffsetFor` is a function of two
rectangles; `lib/useScrollIntoView` is the thin DOM shim over it. jsdom reports
zero layout for everything, so a component that called `el.scrollIntoView()`
would be permanently untestable — and "the current move is visible" is precisely
the behaviour that needs a regression test. The scroll is suppressed when the
selection came from a click inside the panel: yanking the list out from under
the cursor right after a click is worse than leaving it.

## Consequences

- `MoveTree.vue` and `MoveTreeLine.vue` are deleted; `MoveTreePanel` +
  `MoveTreeRow` + `NodeContextMenu` replace them across all three views.
- `MoveTree.test.ts`'s assertion that comments are *not* rendered inline is
  inverted, deliberately, in `MoveTreePanel.test.ts`.
- Copy FEN / Copy line as PGN are defined on the menu but gated behind
  `copyActions` (default off) until the clipboard helpers land — a menu item
  that silently does nothing is the same defect as the piece-set setting that is
  persisted through the API and read by no component.
- Both cycle-guarded walks were added after a test caught that the original
  `mainlinePath` grew its array until `RangeError` on a malformed
  `children[0]` loop.
- New tokens `--depth-1..5`, `--eval-white`, `--eval-black`, `--master` are
  wired at the same time, so `EvalBar`'s hardcoded `neutral-800`/`neutral-100`
  and the master-move legend's `violet-600` stop reading wrong in light mode.
