# 0041 — "Clear arrows" targets the live engine overlays, not hand-drawn shapes

**Context.** Issue #190 (part of #200): the "Clear arrows" board control
(ADR-0024) cleared the wrong layer. It called `Board.clearUserShapes`
(`cg.setShapes([])`), which only touches the hand-drawn right-click layer — the
generated overlays users actually wanted gone (Plans/Threats/Master, and the
Study editor's danger overlay) stayed on screen, and even if cleared they would
instantly reload on the next engine `info` line or FEN change since nothing
turned off their source toggle. Separately, two bugs compounded this: in the
Study editor the button visually cleared the node's *persisted* annotation
shapes without persisting the clear (`cg.setShapes([])` never fires
chessground's `onChange`, so `PUT .../shapes` never ran — they reappeared on
the next navigation); and hand-drawn arrows were silently wiped on every ply
step in Analysis/Games, because `Board.vue`'s `cg.set(config())` always carries
`fen`, and chessground resets its drawable-shapes layer whenever `config.fen`
is set.

**Decision.**

- **"Clear arrows" clears the live engine-analysis overlays and keeps them
  off.** `lib/useBoardOverlays.ts` gained `clear()`: it flips
  `showPlans`/`showThreats`/`showMasterMoves` off via `stores/settings.ts` (one
  `update()` call) and clears the position-derived stores directly
  (`stores/overlays.ts`). Because `lib/boardShapes.ts`'s `composeBoardShapes`
  already gates every layer on its toggle, this empties the board reactively —
  no imperative `cg.setAutoShapes([])` needed from the parent. The layers stay
  off until the user flips a toggle back on; nothing auto-re-enables them.
- **The Study editor folds its danger overlay into the same clear, without
  discarding the walked tree.** `stores/danger.ts`'s `DangerTree` (ADR-0026)
  has no persisted toggle of its own, so `StudyView` tracks a local
  `dangerVisible` ref: `clearArrows()` sets it false, and it resets to `true`
  whenever a fresh walk lands (`watch(() => danger.tree, ...)`). This hides the
  arrows without clearing the side panel's role list — re-running the walk
  (the existing, only way to populate the tree) naturally re-enables it.
- **Persisted annotation shapes are a different concern — "Clear arrows" never
  touches them.** `Board.vue`'s `clearUserShapes()`/`defineExpose` are removed
  outright: no path exists anymore that visually clears a study node's pinned
  shapes without persisting the clear. That control is the existing "Clear
  pinned plan" button (`editor.setShapes([])`, a real, persisted store action);
  bulk annotation-layer clearing is issue #191's concern.
- **Hand-drawn arrows survive ply navigation.** `Board.vue` saves
  `cg.state.drawable.shapes` before every `cg.set(config())` call (in the
  fen/orientation/dests/movable/lastMove watcher) and restores it after, since
  `config()` never sets `drawable.shapes` itself. This is a pure Board.vue fix,
  orthogonal to the "Clear arrows" control — hand-drawn arrows are a
  session-only layer with no clear/persist path of their own.
- **Board hands chessground a copy of pinned shapes, not the live Pinia
  array.** `StudyView`'s `pinnedShapes` now spreads `editor.currentNode.shapes`
  into a new array before it reaches `Board`; chessground mutates
  `drawable.shapes` in place while drawing, and that must never alias
  Pinia-held state.

**Consequences.** The button is relabeled "Clear engine arrows" with an
accurate tooltip. No settings/DB migration: reuses the existing
`show_plans`/`show_threats`/`show_master_moves` toggles (ADR-0024) and the
existing danger-map/pinned-shapes actions. `AnalysisView`/`GamesView` no longer
need a `boardRef` at all — the composable's `clear()` is wired straight to
`@clear-arrows`.
