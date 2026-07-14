import { describe, it, expect } from 'vitest'
import {
  QUERY_FIELDS,
  emptyQuery,
  isEmptyQuery,
  toParams,
  toQueryString,
} from './headerQuery'

describe('emptyQuery', () => {
  it('has every field present and blank', () => {
    const q = emptyQuery()
    expect(Object.keys(q).sort()).toEqual([...QUERY_FIELDS].sort())
    expect(Object.values(q).every((v) => v === '')).toBe(true)
  })

  it('returns a fresh object each time', () => {
    const a = emptyQuery()
    a.player = 'Carlsen'
    expect(emptyQuery().player).toBe('')
  })
})

describe('isEmptyQuery', () => {
  it('is true for a blank query', () => {
    expect(isEmptyQuery(emptyQuery())).toBe(true)
  })

  it('treats whitespace-only fields as blank', () => {
    expect(isEmptyQuery({ ...emptyQuery(), player: '   ' })).toBe(true)
  })

  it('is false once a field carries a value', () => {
    expect(isEmptyQuery({ ...emptyQuery(), event: 'Candidates' })).toBe(false)
  })
})

describe('toParams', () => {
  it('drops blank fields entirely', () => {
    expect(toParams(emptyQuery())).toEqual({})
  })

  it('trims values and keeps only non-blank ones', () => {
    const q = { ...emptyQuery(), player: '  Carlsen  ', event: '   ' }
    expect(toParams(q)).toEqual({ player: 'Carlsen' })
  })

  it('maps camelCase date fields to snake_case params', () => {
    const q = { ...emptyQuery(), dateFrom: '1990.01.01', dateTo: '2000.12.31' }
    expect(toParams(q)).toEqual({ date_from: '1990.01.01', date_to: '2000.12.31' })
  })

  it('maps database/ELO fields to snake_case params and passes sort through', () => {
    const q = {
      ...emptyQuery(),
      databaseId: '3',
      eloMin: ' 2400 ',
      eloMax: '2800',
      sort: 'elo',
    }
    expect(toParams(q)).toEqual({
      database_id: '3',
      elo_min: '2400',
      elo_max: '2800',
      sort: 'elo',
    })
  })

  it('omits an unset database, ELO bound and sort entirely', () => {
    const q = { ...emptyQuery(), eloMin: '2400' }
    expect(toParams(q)).toEqual({ elo_min: '2400' })
  })

  it('passes through the full set of fields', () => {
    const q = {
      player: 'A',
      color: 'white',
      event: 'D',
      result: '1-0',
      eco: 'B90',
      dateFrom: '2020.01.01',
      dateTo: '2021.01.01',
      databaseId: '7',
      eloMin: '2000',
      eloMax: '2500',
      sort: 'id',
    }
    expect(toParams(q)).toEqual({
      player: 'A',
      color: 'white',
      event: 'D',
      result: '1-0',
      eco: 'B90',
      date_from: '2020.01.01',
      date_to: '2021.01.01',
      database_id: '7',
      elo_min: '2000',
      elo_max: '2500',
      sort: 'id',
    })
  })
})

describe('toQueryString', () => {
  it('encodes params, dropping blanks', () => {
    const q = { ...emptyQuery(), result: '1/2-1/2' }
    expect(toQueryString(q)).toBe('result=1%2F2-1%2F2')
  })

  it('is empty for a blank query', () => {
    expect(toQueryString(emptyQuery())).toBe('')
  })
})
