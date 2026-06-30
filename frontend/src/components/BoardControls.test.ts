import { describe, it, expect, vi, beforeEach } from 'vitest'
import { mount } from '@vue/test-utils'
import { setActivePinia, createPinia } from 'pinia'
import BoardControls from './BoardControls.vue'
import { useSettingsStore } from '../stores/settings'

beforeEach(() => {
  window.localStorage.clear()
  setActivePinia(createPinia())
})

describe('BoardControls', () => {
  it('emits navigation events for each nav button', async () => {
    const wrapper = mount(BoardControls, { props: { atStart: false, atEnd: false } })
    await wrapper.find('[aria-label="Start"]').trigger('click')
    await wrapper.find('[aria-label="Back"]').trigger('click')
    await wrapper.find('[aria-label="Forward"]').trigger('click')
    await wrapper.find('[aria-label="End"]').trigger('click')
    expect(wrapper.emitted('first')).toHaveLength(1)
    expect(wrapper.emitted('prev')).toHaveLength(1)
    expect(wrapper.emitted('next')).toHaveLength(1)
    expect(wrapper.emitted('last')).toHaveLength(1)
  })

  it('disables the start/back buttons at the start and forward/end at the end', () => {
    const atStart = mount(BoardControls, { props: { atStart: true, atEnd: false } })
    expect(atStart.find('[aria-label="Start"]').attributes('disabled')).toBeDefined()
    expect(atStart.find('[aria-label="Back"]').attributes('disabled')).toBeDefined()
    expect(atStart.find('[aria-label="Forward"]').attributes('disabled')).toBeUndefined()

    const atEnd = mount(BoardControls, { props: { atStart: false, atEnd: true } })
    expect(atEnd.find('[aria-label="Forward"]').attributes('disabled')).toBeDefined()
    expect(atEnd.find('[aria-label="End"]').attributes('disabled')).toBeDefined()
    expect(atEnd.find('[aria-label="Back"]').attributes('disabled')).toBeUndefined()
  })

  it('reflects the persisted overlay toggles and writes changes to settings', async () => {
    const settings = useSettingsStore()
    const update = vi.spyOn(settings, 'update').mockResolvedValue()
    const wrapper = mount(BoardControls, { props: { atStart: false, atEnd: false } })

    // Defaults: plans on, threats/master off.
    expect((wrapper.find('[data-test="toggle-plans"]').element as HTMLInputElement).checked).toBe(true)
    expect((wrapper.find('[data-test="toggle-threats"]').element as HTMLInputElement).checked).toBe(false)

    await wrapper.find('[data-test="toggle-threats"]').setValue(true)
    expect(update).toHaveBeenCalledWith({ showThreats: true })

    await wrapper.find('[data-test="toggle-plans"]').setValue(false)
    expect(update).toHaveBeenCalledWith({ showPlans: false })
  })

  it('emits clear-arrows', async () => {
    const wrapper = mount(BoardControls, { props: { atStart: false, atEnd: false } })
    await wrapper.find('[data-test="clear-arrows"]').trigger('click')
    expect(wrapper.emitted('clear-arrows')).toHaveLength(1)
  })
})
