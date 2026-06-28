import { describe, it, expect, vi, beforeEach } from 'vitest'
import { mount } from '@vue/test-utils'
import { createPinia } from 'pinia'
import { createRouter, createMemoryHistory } from 'vue-router'
import App from './App.vue'
import { routes } from './router/index.js'

// The board, analysis panel and network live behind the router-view, not in the
// shell — stub the API so settings.load() in App's onMounted stays offline-safe.
vi.mock('./api.js', () => ({
  api: {
    settings: { get: vi.fn().mockResolvedValue({}) },
    health: vi.fn().mockResolvedValue({ status: 'ok', mode: 'local' }),
    databases: { list: vi.fn().mockResolvedValue([]) },
  },
}))

function makeRouter() {
  return createRouter({ history: createMemoryHistory(), routes })
}

describe('App shell', () => {
  let router

  beforeEach(async () => {
    router = makeRouter()
    // Start on a lightweight stub view so the heavy chessground board (which
    // needs real layout) stays out of the shell tests.
    await router.push('/search')
    await router.isReady()
  })

  it('renders the title and the top-level nav', () => {
    const wrapper = mount(App, { global: { plugins: [createPinia(), router] } })
    expect(wrapper.text()).toContain('chess-base')
    const labels = wrapper.findAll('a').map((a) => a.text())
    expect(labels).toEqual(
      expect.arrayContaining(['Analysis', 'Collections', 'Games', 'Search', 'Settings', 'Sign in']),
    )
  })

  it('switches views when navigating', async () => {
    const wrapper = mount(App, { global: { plugins: [createPinia(), router] } })
    expect(wrapper.text()).toContain('Search')

    await router.push('/collections')
    await wrapper.vm.$nextTick()
    expect(wrapper.text()).toContain('Collections')

    await router.push('/login')
    await wrapper.vm.$nextTick()
    expect(wrapper.text()).toContain('Sign in')
  })
})
