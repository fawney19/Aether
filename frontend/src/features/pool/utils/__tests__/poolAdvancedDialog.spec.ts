import { describe, expect, it } from 'vitest'

import {
  buildPoolCooldownFieldLayout,
  buildPoolHealthToggleCards,
  buildPoolCostFieldLayout,
  buildPoolSecondarySectionLayout,
} from '@/features/pool/utils/poolAdvancedDialog'

describe('poolAdvancedDialog', () => {
  it('returns health toggle cards in the desktop display order', () => {
    expect(buildPoolHealthToggleCards().map(item => item.key)).toEqual([
      'health_policy_enabled',
      'probing_enabled',
      'auto_remove_banned_keys',
    ])
  })

  it('provides tooltip copy for every desktop health toggle card', () => {
    expect(buildPoolHealthToggleCards()).toEqual([
      {
        key: 'health_policy_enabled',
        label: '健康策略',
        description: '按上游错误自动冷却并跳过异常账号。',
      },
      {
        key: 'probing_enabled',
        label: '主动探测',
        description: '按固定间隔刷新 Key 的状态与额度，减少号池状态滞后。',
      },
      {
        key: 'auto_remove_banned_keys',
        label: '异常自动清除',
        description: '仅在检测到不可恢复的账号异常时自动从号池移除，不处理纯 Token 失效。',
      },
    ])
  })

  it('returns the four cooldown-related fields in one desktop row order', () => {
    expect(buildPoolCooldownFieldLayout()).toEqual({
      fields: [
        'rate_limit_cooldown_seconds',
        'overload_cooldown_seconds',
        'sticky_session_ttl_seconds',
        'global_priority',
      ],
      desktopColumnsClass: 'xl:grid-cols-4',
    })
  })

  it('stacks batch and cost sections as full-width rows on desktop', () => {
    expect(buildPoolSecondarySectionLayout()).toEqual({
      wrapperClass: 'space-y-4',
    })
  })

  it('returns the three cost fields in one desktop row order', () => {
    expect(buildPoolCostFieldLayout()).toEqual({
      fields: [
        'cost_window_seconds',
        'cost_limit_per_key_tokens',
        'cost_soft_threshold_percent',
      ],
      desktopColumnsClass: 'xl:grid-cols-3',
    })
  })
})
