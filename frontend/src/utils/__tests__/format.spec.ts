import { describe, expect, it } from 'vitest'

import { formatCompactNumber, formatTokens, formatUsageCount } from '../format'

describe('format utils', () => {
  it('formats compact numbers beyond millions', () => {
    expect(formatCompactNumber(999)).toBe('999')
    expect(formatCompactNumber(1_250)).toBe('1.25K')
    expect(formatCompactNumber(12_500_000)).toBe('12.5M')
    expect(formatCompactNumber(1_250_000_000)).toBe('1.25B')
    expect(formatCompactNumber(12_500_000_000_000)).toBe('12.5T')
  })

  it('uses the same B/T compact units for tokens and usage counts', () => {
    expect(formatTokens(1_000_000_000)).toBe('1B')
    expect(formatTokens(1_500_000_000_000)).toBe('1.5T')
    expect(formatUsageCount(1_000_000_000)).toBe('1B')
  })
})
