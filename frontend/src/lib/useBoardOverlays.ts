// Shared board-overlay wiring (issue #134): drives the position-derived overlay
// layers (Threats, Master moves) off a board's live FEN and composes them with
// the engine Plans layer into the union the board renders. Lifted out of
// `AnalysisView.vue` so every board page gets the same overlay behavior.

import { computed, watch } from 'vue'
import { composeBoardShapes } from './boardShapes'
import { useEngineStore } from '../stores/engine'
import { useSettingsStore } from '../stores/settings'
import { useOverlaysStore } from '../stores/overlays'

/** `fen` is a getter so the overlays follow whichever board the caller drives. */
export function useBoardOverlays(fen: () => string) {
  const engine = useEngineStore()
  const settings = useSettingsStore()
  const overlays = useOverlaysStore()

  // The board shows the union of the enabled overlay layers (issue #123): the
  // engine Plans overlay, the Threats arrows and the database master moves —
  // each gated by its persisted setting, composed in one place.
  const boardShapes = computed(() =>
    composeBoardShapes(
      { plans: engine.shapes, threats: overlays.threats, master: overlays.master },
      {
        plans: settings.showPlans,
        threats: settings.showThreats,
        master: settings.showMasterMoves,
      },
    ),
  )

  // (Re)load the position-derived layers when the position or their toggle
  // changes; clear a layer the moment it is switched off so stale arrows never
  // linger.
  watch(
    [fen, () => settings.showThreats, () => settings.showMasterMoves],
    () => {
      const f = fen()
      if (settings.showThreats) overlays.loadThreats(f)
      else overlays.clearThreats()
      if (settings.showMasterMoves) overlays.loadMaster(f)
      else overlays.clearMaster()
    },
    { immediate: true },
  )

  return { boardShapes }
}
