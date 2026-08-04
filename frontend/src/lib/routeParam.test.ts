import { describe, it, expect } from 'vitest'
import { numericParam } from './routeParam'

describe('numericParam', () => {
  it('parses a plain numeric string', () => {
    expect(numericParam('42')).toBe(42)
    expect(numericParam('0')).toBe(0)
  })

  it('takes the first entry of a repeated param', () => {
    expect(numericParam(['7', '8'])).toBe(7)
  })

  it('rejects everything else', () => {
    expect(numericParam(undefined)).toBe(null)
    expect(numericParam('')).toBe(null)
    expect(numericParam('abc')).toBe(null)
    expect(numericParam('1.5')).toBe(null)
    expect(numericParam('-3')).toBe(null)
    expect(numericParam([])).toBe(null)
  })
})
