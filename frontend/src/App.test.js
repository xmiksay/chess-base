import { describe, it, expect, vi } from 'vitest'
import { mount } from '@vue/test-utils'
import { createPinia } from 'pinia'
import App from './App.vue'

// Avoid mounting the real chessground board (needs layout), the engine
// WebSocket, and the network.
vi.mock('./components/Board.vue', () => ({ default: { template: '<div class="board-stub" />' } }))
vi.mock('./components/AnalysisPanel.vue', () => ({ default: { template: '<div class="panel-stub" />' } }))
vi.mock('./api.js', () => ({ api: { health: vi.fn().mockResolvedValue({ status: 'ok', mode: 'local' }) } }))

describe('App', () => {
  it('renders the title and a board', async () => {
    const wrapper = mount(App, { global: { plugins: [createPinia()] } })
    expect(wrapper.text()).toContain('chess-base')
    expect(wrapper.find('.board-stub').exists()).toBe(true)
  })
})
