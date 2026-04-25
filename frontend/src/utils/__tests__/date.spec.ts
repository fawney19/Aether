import { describe, expect, it } from 'vitest'
import { dateTimeLocalToRfc3339, formatDateTimeLocalInput, parseDateLike } from '../date'

function padDatePart(value: number): string {
  return String(value).padStart(2, '0')
}

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

describe('datetime-local conversion', () => {
  it('formats RFC3339 instants for datetime-local inputs using local clock fields', () => {
    const date = new Date('2026-04-12T15:30:00Z')
    const expected = [
      date.getFullYear(),
      padDatePart(date.getMonth() + 1),
      padDatePart(date.getDate()),
    ].join('-') + `T${padDatePart(date.getHours())}:${padDatePart(date.getMinutes())}`

    expect(formatDateTimeLocalInput('2026-04-12T15:30:00Z')).toBe(expected)
  })

  it('converts datetime-local values to RFC3339 instants', () => {
    expect(dateTimeLocalToRfc3339('2026-04-12T15:30')).toBe(
      new Date(2026, 3, 12, 15, 30).toISOString(),
    )
  })

  it('omits blank datetime-local values', () => {
    expect(dateTimeLocalToRfc3339('')).toBeUndefined()
    expect(formatDateTimeLocalInput(undefined)).toBe('')
  })
})
