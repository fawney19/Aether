import { describe, expect, it } from 'vitest'

import {
  buildCustomQuotaSnapshotFromExtractorData,
  getCodexPrimaryQuotaWindow,
  getCodexQuotaDisplayFromSnapshot,
  getCodexQuotaWindowLimitLabel,
  getCodexQuotaWindowPresentation,
} from '../codexQuotaWindow'

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

  it('distinguishes account and model quotas with the same duration', () => {
    const windows = [
      { code: 'weekly', label: '周', scope: 'account', window_minutes: 10_080 },
      { code: 'additional_0_primary', label: 'gpt-reserve', scope: 'model', window_minutes: 10_080 },
    ]

    expect(windows.map(window => getCodexQuotaWindowPresentation(window)?.label))
      .toEqual(['周', 'gpt-reserve 周'])
    expect(getCodexQuotaWindowLimitLabel(windows[1])).toBe('gpt-reserve 周限额')
  })

  it.each([
    [300, 'gpt-reserve 5H'],
    [10_080, 'gpt-reserve 周'],
    [43_800, 'gpt-reserve 月'],
  ])('includes the model name for a %i-minute window', (windowMinutes, expectedLabel) => {
    expect(getCodexQuotaWindowPresentation({
      code: 'additional_0_primary',
      label: ' gpt-reserve ',
      scope: ' Model ',
      window_minutes: windowMinutes,
    })?.label).toBe(expectedLabel)
  })

  it.each(['model', 'quota_group_label', 'quota_group'] as const)(
    'falls back to %s when a model quota label is blank',
    (field) => {
      expect(getCodexQuotaWindowPresentation({
        code: 'additional_0_primary',
        label: ' ',
        scope: 'model',
        [field]: 'gpt-reserve',
        window_minutes: 10_080,
      })?.label).toBe('gpt-reserve 周')
    },
  )

  it('keeps model identities when legacy snapshots have no window duration', () => {
    expect(getCodexQuotaWindowPresentation({
      code: 'additional_0_primary',
      label: 'gpt-reserve',
      scope: 'model',
    })?.label).toBe('gpt-reserve')
    expect(getCodexQuotaWindowPresentation({
      code: 'additional_0_primary',
      model: 'gpt-reserve',
      scope: 'model',
    })?.label).toBe('gpt-reserve')
    expect(getCodexQuotaWindowPresentation({
      code: 'weekly',
      label: 'gpt-reserve',
      scope: 'model',
    })?.label).toBe('gpt-reserve 周')
  })

  it('uses the code when a model quota has no other identity', () => {
    expect(getCodexQuotaWindowPresentation({
      code: 'model-reserve',
      scope: 'model',
      window_minutes: 10_080,
    })?.label).toBe('model-reserve 周')
  })

  it('preserves Spark formatting without stripping other model names', () => {
    expect(getCodexQuotaWindowPresentation({
      code: 'spark_weekly',
      label: 'Spark 周',
      window_minutes: 10_080,
    })).toEqual({ label: 'Spark周', sortOrder: 10_010_080 })
    expect(getCodexQuotaWindowPresentation({ code: 'spark_5h', label: 'Spark 5H' })?.label)
      .toBe('Spark5H')
    expect(getCodexQuotaWindowPresentation({
      code: 'additional_0_primary',
      label: 'Spark Reserve',
      scope: 'model',
      window_minutes: 10_080,
    })?.label).toBe('Spark Reserve 周')
  })

  it('builds the provider limit label from the actual window duration', () => {
    expect(getCodexQuotaWindowLimitLabel({ code: 'weekly', window_minutes: 10_080 })).toBe('周限额')
    expect(getCodexQuotaWindowLimitLabel({ code: 'weekly', window_minutes: 43_800 })).toBe('月限额')
  })

  it('selects a monthly primary window over a zero-minute weekly placeholder', () => {
    const monthly = { code: 'monthly', label: '月', window_minutes: 43_800, used_ratio: 0.02 }
    const selected = getCodexPrimaryQuotaWindow([
      monthly,
      { code: 'weekly', label: '周', window_minutes: 0, used_ratio: 1 },
    ])

    expect(selected).toEqual(monthly)
    expect(getCodexQuotaWindowLimitLabel(selected!)).toBe('月限额')
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

describe('getCodexQuotaDisplayFromSnapshot', () => {
  function extractorData(periods: { key: string, used: number, limit: number, reset_at: string | null }[]) {
    return {
      updated_at: null,
      is_valid: true,
      invalid_message: null,
      remaining: null,
      unit: '%',
      plan_name: null,
      total: null,
      used: null,
      extra: null,
      periods,
    }
  }

  it('projects the weekly window separately when monthly and weekly coexist', () => {
    // Kimi-style custom provider: 5h / week / month all at once.
    const snapshot = buildCustomQuotaSnapshotFromExtractorData(extractorData([
      { key: '5h', used: 0, limit: 1, reset_at: '2026-10-30T00:00:00Z' },
      { key: 'week', used: 0.3, limit: 1, reset_at: '2026-10-29T00:00:00Z' },
      { key: 'month', used: 0.95, limit: 1, reset_at: '2026-11-01T00:00:00Z' },
    ]))
    expect(snapshot).not.toBeNull()
    const display = getCodexQuotaDisplayFromSnapshot(snapshot)!
    // Primary slot takes the monthly window…
    expect(display.primary_code).toBe('monthly')
    expect(display.primary_used_percent).toBe(95)
    // …while the weekly window gets its own fields instead of being dropped.
    expect(display.weekly_used_percent).toBe(30)
    expect(display.secondary_used_percent).toBe(0)
  })

  it('does not duplicate the weekly row when weekly is the primary window', () => {
    const snapshot = buildCustomQuotaSnapshotFromExtractorData(extractorData([
      { key: '5h', used: 0, limit: 1, reset_at: null },
      { key: 'week', used: 0.3, limit: 1, reset_at: null },
    ]))
    const display = getCodexQuotaDisplayFromSnapshot(snapshot)!
    expect(display.primary_code).toBe('weekly')
    expect(display.primary_used_percent).toBe(30)
    expect(display.weekly_used_percent).toBe(30)
    // The grid hides the separate weekly row via primary_code === 'weekly'.
  })
})
