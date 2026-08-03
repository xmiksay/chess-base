import { describe, it, expect, vi, beforeEach } from 'vitest'
import { mount } from '@vue/test-utils'
import { setActivePinia, createPinia } from 'pinia'
import StudyAnalysis from './StudyAnalysis.vue'
import EnginePanel from './EnginePanel.vue'
import { useEngineStore } from '../stores/engine'
import { useStudyEditorStore } from '../stores/studyEditor'
import type { AnalyseStats, EngineLine, PlanLine } from '../types'

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

function plan(over: Partial<PlanLine> = {}): PlanLine {
  return {
    multipv: 1,
    depth: 18,
    score: { type: 'cp', value: 35 },
    pv: ['e2e4'],
    trajectories: [{ piece: 'P', squares: ['e2', 'e4'] }],
    ...over,
  }
}

beforeEach(() => {
  window.localStorage.clear()
  setActivePinia(createPinia())
})

describe('StudyAnalysis', () => {
  it('drives the shared EnginePanel with the selected node fen', () => {
    const engine = useEngineStore()
    vi.spyOn(engine, 'connect').mockImplementation(() => {})
    vi.spyOn(engine, 'disconnect').mockImplementation(() => {})
    const editor = useStudyEditorStore()

    const wrapper = mount(StudyAnalysis)
    // The EnginePanel is fed the editor's selected-node position.
    expect(wrapper.findComponent(EnginePanel).props('fen')).toBe(editor.fen)
  })

  it('pins an engine line plan to the current node via the EnginePanel slot', async () => {
    const engine = useEngineStore()
    vi.spyOn(engine, 'connect').mockImplementation(() => {})
    vi.spyOn(engine, 'disconnect').mockImplementation(() => {})
    engine.lines = [line()]
    engine.plans = [plan()]

    const editor = useStudyEditorStore()
    const setShapes = vi.spyOn(editor, 'setShapes').mockResolvedValue()

    const wrapper = mount(StudyAnalysis)
    const pin = wrapper.find('[data-test="pin-line"]')
    expect(pin.exists()).toBe(true)

    await pin.trigger('click')
    expect(setShapes).toHaveBeenCalledWith([{ orig: 'e2', dest: 'e4', brush: 'plan1' }])
  })

  it('surfaces a pin error', async () => {
    const engine = useEngineStore()
    vi.spyOn(engine, 'connect').mockImplementation(() => {})
    vi.spyOn(engine, 'disconnect').mockImplementation(() => {})
    engine.lines = [line()]
    engine.plans = [plan()]

    const editor = useStudyEditorStore()
    vi.spyOn(editor, 'setShapes').mockRejectedValue(new Error('nope'))

    const wrapper = mount(StudyAnalysis)
    await wrapper.find('[data-test="pin-line"]').trigger('click')
    await wrapper.vm.$nextTick()
    expect(wrapper.find('[data-test="pin-error"]').text()).toContain('nope')
  })

  it('shows the classification roll-up after analysing (#189)', async () => {
    const engine = useEngineStore()
    vi.spyOn(engine, 'connect').mockImplementation(() => {})
    vi.spyOn(engine, 'disconnect').mockImplementation(() => {})

    const editor = useStudyEditorStore()
    const stats: AnalyseStats = {
      nodes_analysed: 4,
      summary: {
        white: { acpl: 12, accuracy: 91.2, inaccuracies: 1, mistakes: 0, blunders: 0 },
        black: { acpl: 40, accuracy: 60.5, inaccuracies: 0, mistakes: 1, blunders: 1 },
      },
    }
    vi.spyOn(editor, 'analyseStudy').mockResolvedValue(stats)

    const wrapper = mount(StudyAnalysis)
    expect(wrapper.find('[data-test="analyse-stats"]').exists()).toBe(false)

    await wrapper.find('[data-test="analyse-study"]').trigger('click')
    await wrapper.vm.$nextTick()

    expect(wrapper.find('[data-test="analyse-stats-white"]').text()).toContain('91.2%')
    expect(wrapper.find('[data-test="analyse-stats-black"]').text()).toContain('60.5%')
    expect(wrapper.find('[data-test="analyse-stats-black"]').text()).toContain('1 ??')
  })

  it('surfaces an analyse error', async () => {
    const engine = useEngineStore()
    vi.spyOn(engine, 'connect').mockImplementation(() => {})
    vi.spyOn(engine, 'disconnect').mockImplementation(() => {})

    const editor = useStudyEditorStore()
    vi.spyOn(editor, 'analyseStudy').mockRejectedValue(new Error('no engine'))

    const wrapper = mount(StudyAnalysis)
    await wrapper.find('[data-test="analyse-study"]').trigger('click')
    await wrapper.vm.$nextTick()
    expect(wrapper.find('[data-test="analyse-error"]').text()).toContain('no engine')
  })
})
