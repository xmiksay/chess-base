// Bridges the persistent engine options (settings store, per-user) with the live
// engine store the analysis socket reads. The settings store is the source of
// truth: it seeds the engine store on mount and whenever the persisted values
// change (e.g. after `settings.load()` hydrates from the server), and `persist()`
// writes a control edit back through the settings store, then re-issues the
// search so the new MultiPV/Threads/Hash take effect immediately.

import { watch } from 'vue'
import { useEngineStore } from '../stores/engine'
import { useSettingsStore } from '../stores/settings'

export function useEnginePrefs() {
  const engine = useEngineStore()
  const settings = useSettingsStore()

  function seed() {
    engine.multipv = settings.engineMultipv
    engine.threads = settings.engineThreads
    engine.hash = settings.engineHash
  }
  seed()
  watch(
    () => [settings.engineMultipv, settings.engineThreads, settings.engineHash],
    seed,
  )

  /** Persist the engine store's current options and restart the search. */
  function persist() {
    settings.update({
      engineMultipv: engine.multipv,
      engineThreads: engine.threads,
      engineHash: engine.hash,
    })
    engine.reconfigure()
  }

  return { persist }
}
