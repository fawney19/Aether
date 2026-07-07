import { describe, expect, it } from 'vitest'

import {
  formatCodexResetCreditCount,
  formatCodexResetCreditDays,
  getCodexResetCreditAvailableCount,
  getVisibleCodexResetCreditItems,
} from '@/features/providers/components/codex-reset-credit-display'
import type { QuotaResetCreditsSnapshot } from '@/api/endpoints/types'

describe('codex reset credit display helpers', () => {
  it('keeps zero available credits displayable but non-positive detail items hidden', () => {
    const snapshot: QuotaResetCreditsSnapshot = {
      available_count: 0,
      updated_at: 1_700_000_000,
      credits: [
        {
          id: 'expired-1111',
          display_key: 'expired',
          status: 'available',
          expires_at: 1_699_999_999,
        },
      ],
    }

    expect(getCodexResetCreditAvailableCount(snapshot)).toBe(0)
    expect(formatCodexResetCreditCount(0)).toBe('共 0 次机会')
    expect(getVisibleCodexResetCreditItems(snapshot, 1_700_000_000)).toEqual([])
  })

  it('sorts available detail items by remaining time and labels visible items with short ordinal keys', () => {
    const snapshot: QuotaResetCreditsSnapshot = {
      available_count: 7,
      updated_at: 1_700_000_000,
      credits: [
        { id: 'sixth-0000', status: 'available', expires_at: 1_700_060_000 },
        { id: 'spent-0000', status: 'redeemed', expires_at: 1_700_001_000 },
        { id: 'fifth-0000', status: 'active', expires_at: 1_700_050_000 },
        { id: 'third-0000', status: 'available', remaining_seconds: 30_000 },
        { id: 'missing-expiry-0000', status: 'available' },
        { id: 'first-0000', status: 'available', expires_at: 1_700_010_000 },
        { id: 'second-0000', status: 'available', expires_at: 1_700_020_000 },
        {
          id: 'fourth-0000',
          display_key: 'RateLimitResetCredit_05cbb6eeeb9c81918e011d8300f9ebfb',
          status: 'available',
          expires_at: 1_700_040_000,
        },
      ],
    }

    const items = getVisibleCodexResetCreditItems(snapshot, 1_700_000_000)

    expect(items.map(item => item.displayKey)).toEqual([
      'Key-1',
      'Key-2',
      'Key-3',
      'Key-4',
      'Key-5',
    ])
    expect(items.map(item => item.title)).toEqual([
      'Codex 重置机会 Key-1',
      'Codex 重置机会 Key-2',
      'Codex 重置机会 Key-3',
      'Codex 重置机会 Key-4',
      'Codex 重置机会 Key-5',
    ])
    expect(items.map(item => item.remainingSeconds)).toEqual([
      10_000,
      20_000,
      30_000,
      40_000,
      50_000,
    ])
  })

  it('formats reset credit remaining days with a one-day minimum', () => {
    expect(formatCodexResetCreditDays(1)).toBe('1天')
    expect(formatCodexResetCreditDays(86_401)).toBe('2天')
  })
})
