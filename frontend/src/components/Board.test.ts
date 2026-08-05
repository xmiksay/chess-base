import { describe, it, expect, vi } from 'vitest'
import { mount } from '@vue/test-utils'

// A minimal fake of chessground's `Api`, just enough to exercise Board.vue's
// own set/restore logic (issue #190). `set()` mirrors chessground's real
// `configure()`: any `config.fen` resets `state.drawable.shapes` to
// `config.drawable?.shapes ?? []` — the behavior that used to wipe hand-drawn
// arrows on every ply step.
function makeFakeApi() {
  // Registered brushes (a slice of chessground defaults + app brushes) — Board
  // filters shapes against this table before rendering (issue #228).
  const state = {
    drawable: { shapes: [] as unknown[], brushes: { green: {}, blue: {}, plan1: {} } },
  }
  return {
    state,
    set: vi.fn((config: { fen?: string; drawable?: { shapes?: unknown[] } }) => {
      if (config.fen) state.drawable.shapes = config.drawable?.shapes ?? []
    }),
    setShapes: vi.fn((shapes: unknown[]) => {
      state.drawable.shapes = shapes
    }),
    setAutoShapes: vi.fn(),
  }
}

let fakeApi: ReturnType<typeof makeFakeApi>
vi.mock('chessground', () => ({
  Chessground: vi.fn(() => fakeApi),
}))

import Board from './Board.vue'
import { STARTPOS_FEN } from '../lib/fen'

const FEN_AFTER_E4 = 'rnbqkbnr/pppppppp/8/8/4P3/8/PPPP1PPP/RNBQKBNR b KQkq - 0 1'

describe('Board', () => {
  it('preserves hand-drawn arrows across a ply step (issue #190)', async () => {
    fakeApi = makeFakeApi()
    const wrapper = mount(Board, { props: { fen: STARTPOS_FEN } })

    // Simulate a right-click-drawn arrow: chessground's own draw.ts mutates
    // `state.drawable.shapes` directly — it never round-trips through a prop.
    fakeApi.state.drawable.shapes = [{ orig: 'e2', dest: 'e4', brush: 'green' }]

    await wrapper.setProps({ fen: FEN_AFTER_E4 })

    expect(fakeApi.state.drawable.shapes).toEqual([{ orig: 'e2', dest: 'e4', brush: 'green' }])
  })

  it('does not touch the drawn-shapes layer when only the auto-shape overlay changes', async () => {
    fakeApi = makeFakeApi()
    const wrapper = mount(Board, { props: { fen: STARTPOS_FEN, shapes: [] } })
    fakeApi.state.drawable.shapes = [{ orig: 'g1', dest: 'f3', brush: 'blue' }]

    await wrapper.setProps({ shapes: [{ orig: 'e2', dest: 'e4', brush: 'plan1' }] })

    expect(fakeApi.setAutoShapes).toHaveBeenCalledWith([{ orig: 'e2', dest: 'e4', brush: 'plan1' }])
    expect(fakeApi.state.drawable.shapes).toEqual([{ orig: 'g1', dest: 'f3', brush: 'blue' }])
  })

  it('drops a shape with an unregistered brush instead of blanking the layer (issue #228)', async () => {
    fakeApi = makeFakeApi()
    const wrapper = mount(Board, { props: { fen: STARTPOS_FEN, shapes: [] } })

    await wrapper.setProps({
      shapes: [
        { orig: 'g2', dest: 'a8', brush: 'cyan' }, // unknown — chessground would throw
        { orig: 'e2', dest: 'e4', brush: 'green' },
      ],
    })

    expect(fakeApi.setAutoShapes).toHaveBeenCalledWith([{ orig: 'e2', dest: 'e4', brush: 'green' }])
  })
})
