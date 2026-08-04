import { describe, it, expect, vi, beforeEach } from 'vitest'
import { mount } from '@vue/test-utils'
import ShareToggle from './ShareToggle.vue'

describe('ShareToggle', () => {
  beforeEach(() => {
    Object.assign(navigator, { clipboard: { writeText: vi.fn().mockResolvedValue(undefined) } })
  })

  it('emits update:modelValue when the checkbox is toggled', async () => {
    const wrapper = mount(ShareToggle, {
      props: { modelValue: false, shareUrl: 'https://example.test/games/5' },
    })
    await wrapper.find('[data-test="public-toggle"]').setValue(true)
    expect(wrapper.emitted('update:modelValue')).toEqual([[true]])
  })

  it('hides the link/copy affordance while private', () => {
    const wrapper = mount(ShareToggle, {
      props: { modelValue: false, shareUrl: 'https://example.test/games/5' },
    })
    expect(wrapper.find('[data-test="share-link"]').exists()).toBe(false)
    expect(wrapper.find('[data-test="copy-link"]').exists()).toBe(false)
  })

  it('shows the share link and copies it to the clipboard once public', async () => {
    const wrapper = mount(ShareToggle, {
      props: { modelValue: true, shareUrl: 'https://example.test/games/5' },
    })
    expect((wrapper.find('[data-test="share-link"]').element as HTMLInputElement).value).toBe(
      'https://example.test/games/5',
    )
    await wrapper.find('[data-test="copy-link"]').trigger('click')
    expect(navigator.clipboard.writeText).toHaveBeenCalledWith('https://example.test/games/5')
  })

  it('disables the checkbox while a toggle is in flight', () => {
    const wrapper = mount(ShareToggle, {
      props: { modelValue: false, shareUrl: 'https://example.test/games/5', disabled: true },
    })
    expect((wrapper.find('[data-test="public-toggle"]').element as HTMLInputElement).disabled).toBe(true)
  })
})
