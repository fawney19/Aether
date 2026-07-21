import { describe, expect, it } from 'vitest'

import { getCodexQuotaWindowPresentation } from '../codexQuotaWindow'

describe('getCodexQuotaWindowPresentation', () => {
  it.each([
    [300, '5H'],
    [10_080, '周'],
    [43_200, '月'],
    [43_800, '月'],
    [44_640, '月'],
  ])('labels a %i-minute window as %s', (windowMinutes, expectedLabel) => {
    expect(getCodexQuotaWindowPresentation({
      code: 'primary',
      window_minutes: windowMinutes,
    })?.label).toBe(expectedLabel)
  })

  it('supports simultaneous 5H and weekly windows', () => {
    const windows = [
      getCodexQuotaWindowPresentation({ code: 'secondary', window_minutes: 10_080 }),
      getCodexQuotaWindowPresentation({ code: 'primary', window_minutes: 300 }),
    ].filter((item): item is NonNullable<typeof item> => item != null)

    expect(windows.sort((a, b) => a.sortOrder - b.sortOrder).map(item => item.label)).toEqual(['5H', '周'])
  })

  it('drops zero-minute placeholder windows', () => {
    expect(getCodexQuotaWindowPresentation({
      code: 'weekly',
      label: '周',
      window_minutes: 0,
    })).toBeNull()
  })

  it('keeps legacy labels when old snapshots have no window duration', () => {
    expect(getCodexQuotaWindowPresentation({ code: '5h' })?.label).toBe('5H')
    expect(getCodexQuotaWindowPresentation({ code: 'weekly' })?.label).toBe('周')
  })
})
