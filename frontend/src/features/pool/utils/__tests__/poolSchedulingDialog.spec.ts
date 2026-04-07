import { describe, expect, it } from 'vitest'

import { moveStrategyItem } from '@/features/pool/utils/poolSchedulingDialog'

interface TestPresetItem {
  preset: string
  mutexGroup: string | null
  enabled: boolean
}

function buildItems(): TestPresetItem[] {
  return [
    { preset: 'cache_affinity', mutexGroup: 'distribution_mode', enabled: false },
    { preset: 'lru', mutexGroup: 'distribution_mode', enabled: true },
    { preset: 'single_account', mutexGroup: 'distribution_mode', enabled: false },
    { preset: 'load_balance', mutexGroup: 'distribution_mode', enabled: false },
    { preset: 'recent_refresh', mutexGroup: null, enabled: true },
    { preset: 'quota_balanced', mutexGroup: null, enabled: false },
    { preset: 'priority_first', mutexGroup: null, enabled: true },
  ]
}

describe('poolSchedulingDialog', () => {
  it('moves only strategy items upward without disturbing distribution presets', () => {
    const moved = moveStrategyItem(buildItems(), 6, -1)

    expect(moved.map(item => item.preset)).toEqual([
      'cache_affinity',
      'lru',
      'single_account',
      'load_balance',
      'recent_refresh',
      'priority_first',
      'quota_balanced',
    ])
  })

  it('keeps the original order when a strategy item is already at the top boundary', () => {
    const original = buildItems()
    const moved = moveStrategyItem(original, 4, -1)

    expect(moved.map(item => item.preset)).toEqual(original.map(item => item.preset))
  })

  it('moves a strategy item downward within the strategy group', () => {
    const moved = moveStrategyItem(buildItems(), 4, 1)

    expect(moved.map(item => item.preset)).toEqual([
      'cache_affinity',
      'lru',
      'single_account',
      'load_balance',
      'quota_balanced',
      'recent_refresh',
      'priority_first',
    ])
  })

  it('keeps the original order when a strategy item is already at the bottom boundary', () => {
    const original = buildItems()
    const moved = moveStrategyItem(original, 6, 1)

    expect(moved.map(item => item.preset)).toEqual(original.map(item => item.preset))
  })

  it('keeps the original order when the target item is not a strategy preset', () => {
    const original = buildItems()
    const moved = moveStrategyItem(original, 1, 1)

    expect(moved.map(item => item.preset)).toEqual(original.map(item => item.preset))
  })
})
