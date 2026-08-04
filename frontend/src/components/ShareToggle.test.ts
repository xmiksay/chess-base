import { describe, it, expect, vi, afterEach } from 'vitest'
import { mount } from '@vue/test-utils'
import ShareToggle from './ShareToggle.vue'

function mountToggle(props: { isPublic: boolean; canToggle: boolean; url?: string }) {
  return mount(ShareToggle, { props: { url: 'https://x.test/games/5', ...props } })
}

describe('ShareToggle', () => {
  afterEach(() => vi.restoreAllMocks())

  it('emits toggle when the checkbox changes', async () => {
    const wrapper = mountToggle({ isPublic: false, canToggle: true })
    await wrapper.find('[data-test="share-toggle"]').trigger('change')
    expect(wrapper.emitted('toggle')).toHaveLength(1)
  })

  it('reflects the public state on the checkbox', () => {
    const wrapper = mountToggle({ isPublic: true, canToggle: true })
    const box = wrapper.find('[data-test="share-toggle"]').element as HTMLInputElement
    expect(box.checked).toBe(true)
  })

  it('copies the deep link to the clipboard', async () => {
    const writeText = vi.fn().mockResolvedValue(undefined)
    vi.stubGlobal('navigator', { clipboard: { writeText } })

    const wrapper = mountToggle({ isPublic: true, canToggle: true })
    await wrapper.find('[data-test="copy-link"]').trigger('click')

    expect(writeText).toHaveBeenCalledWith('https://x.test/games/5')
    vi.unstubAllGlobals()
  })

  it('falls back to a prompt when the clipboard is unavailable', async () => {
    vi.stubGlobal('navigator', {}) // no clipboard at all
    const prompt = vi.spyOn(window, 'prompt').mockReturnValue(null)

    const wrapper = mountToggle({ isPublic: true, canToggle: true })
    await wrapper.find('[data-test="copy-link"]').trigger('click')

    expect(prompt).toHaveBeenCalledWith('Copy link', 'https://x.test/games/5')
    vi.unstubAllGlobals()
  })

  it('hides the checkbox when the caller cannot toggle, keeping the pill', () => {
    const wrapper = mountToggle({ isPublic: true, canToggle: false })
    expect(wrapper.find('[data-test="share-toggle"]').exists()).toBe(false)
    expect(wrapper.find('[data-test="share-pill"]').exists()).toBe(true)
    expect(wrapper.find('[data-test="copy-link"]').exists()).toBe(true)
  })

  it('renders nothing actionable for a private object the caller cannot toggle', () => {
    const wrapper = mountToggle({ isPublic: false, canToggle: false })
    expect(wrapper.find('[data-test="share-toggle"]').exists()).toBe(false)
    expect(wrapper.find('[data-test="share-pill"]').exists()).toBe(false)
    expect(wrapper.find('[data-test="copy-link"]').exists()).toBe(false)
  })
})
