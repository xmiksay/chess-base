import { describe, it, expect } from 'vitest'
import { FILTER_FIELDS, emptyFilter, isEmptyFilter, toParams } from './positionFilter'

describe('emptyFilter', () => {
  it('has every field present and blank', () => {
    const f = emptyFilter()
    expect(Object.keys(f).sort()).toEqual([...FILTER_FIELDS].sort())
    expect(Object.values(f).every((v) => v === '')).toBe(true)
  })

  it('returns a fresh object each time', () => {
    const a = emptyFilter()
    a.player = 'Carlsen'
    expect(emptyFilter().player).toBe('')
  })
})

describe('isEmptyFilter', () => {
  it('is true for a blank filter', () => {
    expect(isEmptyFilter(emptyFilter())).toBe(true)
  })

  it('treats whitespace-only fields as blank', () => {
    expect(isEmptyFilter({ ...emptyFilter(), player: '   ' })).toBe(true)
  })

  it('is false once a field carries a value', () => {
    expect(isEmptyFilter({ ...emptyFilter(), color: 'white' })).toBe(false)
  })
})

describe('toParams', () => {
  it('drops blank fields entirely', () => {
    expect(toParams(emptyFilter())).toEqual({})
  })

  it('trims values and keeps only non-blank ones', () => {
    const f = { ...emptyFilter(), player: '  Carlsen  ', color: '   ' }
    expect(toParams(f)).toEqual({ player: 'Carlsen' })
  })

  it('maps camelCase date fields to snake_case params', () => {
    const f = { ...emptyFilter(), dateFrom: '1990.01.01', dateTo: '2000.12.31' }
    expect(toParams(f)).toEqual({ date_from: '1990.01.01', date_to: '2000.12.31' })
  })

  it('passes through the full set of fields', () => {
    const f = {
      player: 'A',
      color: 'white',
      dateFrom: '2020.01.01',
      dateTo: '2021.01.01',
    }
    expect(toParams(f)).toEqual({
      player: 'A',
      color: 'white',
      date_from: '2020.01.01',
      date_to: '2021.01.01',
    })
  })
})
