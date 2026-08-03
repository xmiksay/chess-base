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

/** A renderable stats roll-up for mocking `analyseStudy` resolutions. */
function emptyStats(): AnalyseStats {
  return {
    nodes_analysed: 0,
    summary: {
      white: { acpl: 0, accuracy: 100, inaccuracies: 0, mistakes: 0, blunders: 0 },
      black: { acpl: 0, accuracy: 100, inaccuracies: 0, mistakes: 0, blunders: 0 },
    },
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

  it('submits an empty options body by default, and only depth when set (#216)', async () => {
    const engine = useEngineStore()
    vi.spyOn(engine, 'connect').mockImplementation(() => {})
    vi.spyOn(engine, 'disconnect').mockImplementation(() => {})

    const editor = useStudyEditorStore()
    const analyse = vi.spyOn(editor, 'analyseStudy').mockResolvedValue(emptyStats())

    const wrapper = mount(StudyAnalysis)
    // The shape sub-options stay hidden until the master checkbox is ticked.
    expect(wrapper.find('[data-test="analyse-shape-options"]').exists()).toBe(false)

    await wrapper.find('[data-test="analyse-study"]').trigger('click')
    expect(analyse).toHaveBeenCalledWith({})

    await wrapper.find('[data-test="analyse-depth"]').setValue(14)
    await wrapper.find('[data-test="analyse-study"]').trigger('click')
    expect(analyse).toHaveBeenLastCalledWith({ depth: 14 })
  })

  it('submits plan_lines/threats once opted in — including the explicit 0/false strip case (#191)', async () => {
    const engine = useEngineStore()
    vi.spyOn(engine, 'connect').mockImplementation(() => {})
    vi.spyOn(engine, 'disconnect').mockImplementation(() => {})

    const editor = useStudyEditorStore()
    const analyse = vi.spyOn(editor, 'analyseStudy').mockResolvedValue(emptyStats())

    const wrapper = mount(StudyAnalysis)
    await wrapper.find('[data-test="analyse-regen-shapes"]').setValue(true)
    await wrapper.find('[data-test="analyse-plan-lines"]').setValue(3)
    await wrapper.find('[data-test="analyse-threats"]').setValue(true)
    await wrapper.find('[data-test="analyse-study"]').trigger('click')
    expect(analyse).toHaveBeenLastCalledWith({ plan_lines: 3, threats: true })

    // 0/false is NOT "omit": it opts in and strips the stale generated arrows.
    await wrapper.find('[data-test="analyse-plan-lines"]').setValue(0)
    await wrapper.find('[data-test="analyse-threats"]').setValue(false)
    await wrapper.find('[data-test="analyse-study"]').trigger('click')
    expect(analyse).toHaveBeenLastCalledWith({ plan_lines: 0, threats: false })

    // Unticking the master checkbox omits both fields again.
    await wrapper.find('[data-test="analyse-regen-shapes"]').setValue(false)
    await wrapper.find('[data-test="analyse-study"]').trigger('click')
    expect(analyse).toHaveBeenLastCalledWith({})
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

  it('removes generated arrows without asking for confirmation (#191)', async () => {
    const engine = useEngineStore()
    vi.spyOn(engine, 'connect').mockImplementation(() => {})
    vi.spyOn(engine, 'disconnect').mockImplementation(() => {})
    const confirmSpy = vi.spyOn(window, 'confirm')

    const editor = useStudyEditorStore()
    const clearShapes = vi.spyOn(editor, 'clearShapes').mockResolvedValue()

    const wrapper = mount(StudyAnalysis)
    await wrapper.find('[data-test="clear-generated-shapes"]').trigger('click')

    expect(clearShapes).toHaveBeenCalledWith('generated')
    expect(confirmSpy).not.toHaveBeenCalled()
  })

  it('asks for confirmation before removing all arrows, and skips the call when declined', async () => {
    const engine = useEngineStore()
    vi.spyOn(engine, 'connect').mockImplementation(() => {})
    vi.spyOn(engine, 'disconnect').mockImplementation(() => {})
    vi.spyOn(window, 'confirm').mockReturnValue(false)

    const editor = useStudyEditorStore()
    const clearShapes = vi.spyOn(editor, 'clearShapes').mockResolvedValue()

    const wrapper = mount(StudyAnalysis)
    await wrapper.find('[data-test="clear-all-shapes"]').trigger('click')

    expect(clearShapes).not.toHaveBeenCalled()
  })

  it('removes all arrows once confirmed', async () => {
    const engine = useEngineStore()
    vi.spyOn(engine, 'connect').mockImplementation(() => {})
    vi.spyOn(engine, 'disconnect').mockImplementation(() => {})
    vi.spyOn(window, 'confirm').mockReturnValue(true)

    const editor = useStudyEditorStore()
    const clearShapes = vi.spyOn(editor, 'clearShapes').mockResolvedValue()

    const wrapper = mount(StudyAnalysis)
    await wrapper.find('[data-test="clear-all-shapes"]').trigger('click')

    expect(clearShapes).toHaveBeenCalledWith('all')
  })

  it('marks transpositions via the store action (#174)', async () => {
    const engine = useEngineStore()
    vi.spyOn(engine, 'connect').mockImplementation(() => {})
    vi.spyOn(engine, 'disconnect').mockImplementation(() => {})

    const editor = useStudyEditorStore()
    const mark = vi.spyOn(editor, 'markTranspositions').mockResolvedValue()

    const wrapper = mount(StudyAnalysis)
    await wrapper.find('[data-test="mark-transpositions"]').trigger('click')

    expect(mark).toHaveBeenCalledTimes(1)
  })

  it('surfaces a mark-transpositions error', async () => {
    const engine = useEngineStore()
    vi.spyOn(engine, 'connect').mockImplementation(() => {})
    vi.spyOn(engine, 'disconnect').mockImplementation(() => {})

    const editor = useStudyEditorStore()
    vi.spyOn(editor, 'markTranspositions').mockRejectedValue(new Error('boom'))

    const wrapper = mount(StudyAnalysis)
    await wrapper.find('[data-test="mark-transpositions"]').trigger('click')
    await wrapper.vm.$nextTick()
    expect(wrapper.find('[data-test="mark-transpositions-error"]').text()).toContain('boom')
  })

  it('surfaces a clear-shapes error', async () => {
    const engine = useEngineStore()
    vi.spyOn(engine, 'connect').mockImplementation(() => {})
    vi.spyOn(engine, 'disconnect').mockImplementation(() => {})

    const editor = useStudyEditorStore()
    vi.spyOn(editor, 'clearShapes').mockRejectedValue(new Error('nope'))

    const wrapper = mount(StudyAnalysis)
    await wrapper.find('[data-test="clear-generated-shapes"]').trigger('click')
    await wrapper.vm.$nextTick()
    expect(wrapper.find('[data-test="clear-shapes-error"]').text()).toContain('nope')
  })
})
