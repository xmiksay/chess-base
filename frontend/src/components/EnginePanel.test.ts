import { describe, it, expect, vi, beforeEach } from 'vitest'
import { mount } from '@vue/test-utils'
import { setActivePinia, createPinia } from 'pinia'
import EnginePanel from './EnginePanel.vue'
import { useEngineStore } from '../stores/engine'
import { STARTPOS_FEN } from '../lib/fen'
import type { EngineLine } from '../types'

function line(over: Partial<EngineLine> = {}): EngineLine {
  return {
    multipv: 1,
    depth: 18,
    seldepth: 24,
    score: { type: 'cp', value: 35 },
    nodes: 1000,
    nps: 50000,
    timeMs: 100,
    pv: ['e2e4', 'e7e5'],
    ...over,
  }
}

beforeEach(() => {
  window.localStorage.clear()
  setActivePinia(createPinia())
})

describe('EnginePanel', () => {
  it('renders the eval and PV lines for the given fen', () => {
    const engine = useEngineStore()
    // The panel manages the socket lifecycle; keep it inert in the test.
    vi.spyOn(engine, 'connect').mockImplementation(() => {})
    vi.spyOn(engine, 'disconnect').mockImplementation(() => {})
    engine.lines = [line()]
    engine.depth = 18

    const wrapper = mount(EnginePanel, { props: { fen: STARTPOS_FEN } })
    // +0.35 from White's side to move at the start position.
    expect(wrapper.text()).toContain('+0.35')
    expect(wrapper.text()).toContain('d18')
    // The PV is rendered as SAN against the (start) position.
    expect(wrapper.text()).toContain('e4')
    expect(wrapper.text()).toContain('e5')
  })

  it('analyses by default and stops/restarts with the toggle', async () => {
    const engine = useEngineStore()
    vi.spyOn(engine, 'connect').mockImplementation(() => {})
    vi.spyOn(engine, 'disconnect').mockImplementation(() => {})
    const stop = vi.spyOn(engine, 'stop').mockImplementation(() => {})
    const analyse = vi.spyOn(engine, 'analyse').mockImplementation(() => {})
    // Engine ready at mount → analysis is on by default and auto-starts.
    engine.status = 'ready'

    const wrapper = mount(EnginePanel, { props: { fen: STARTPOS_FEN } })
    const toggle = wrapper.find('[data-test="analyse-toggle"]')
    expect(analyse).toHaveBeenCalledWith(STARTPOS_FEN, {})
    expect((toggle.element as HTMLInputElement).checked).toBe(true)

    await toggle.setValue(false)
    expect(stop).toHaveBeenCalled()

    await toggle.setValue(true)
    expect(analyse).toHaveBeenCalledTimes(2)
  })

  it('hides the analyse toggle and PV list when analysis is delegated away', () => {
    const engine = useEngineStore()
    vi.spyOn(engine, 'connect').mockImplementation(() => {})
    vi.spyOn(engine, 'disconnect').mockImplementation(() => {})
    engine.lines = [line()]

    const wrapper = mount(EnginePanel, { props: { fen: STARTPOS_FEN, analyse: false } })
    expect(wrapper.find('[data-test="analyse-toggle"]').exists()).toBe(false)
    // The eval readout still shows so a play-vs-engine consumer keeps the bar.
    expect(wrapper.text()).toContain('+0.35')
  })
})
