import { describe, expect, it } from 'vitest'
import { parseDateLike } from '../date'

describe('parseDateLike', () => {
  it('parses date-only strings as local calendar dates', () => {
    const date = parseDateLike('2026-04-12')

    expect(date.getFullYear()).toBe(2026)
    expect(date.getMonth()).toBe(3)
    expect(date.getDate()).toBe(12)
  })

  it('keeps timestamp strings delegated to native Date parsing', () => {
    const date = parseDateLike('2026-04-12T15:30:00Z')

    expect(Number.isNaN(date.getTime())).toBe(false)
    expect(date.toISOString()).toBe('2026-04-12T15:30:00.000Z')
  })
})
