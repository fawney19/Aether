import { describe, expect, it } from 'vitest'

import {
  POOL_PRE_PROBE_DISABLED_TOOLTIP,
  POOL_PRE_PROBE_HELP_TEXT,
  buildPoolPreProbeForm,
  buildPoolPreProbePayload,
  buildPoolCooldownFieldLayout,
  buildPoolHealthToggleCards,
  buildPoolCostFieldLayout,
  buildPoolPreProbeFieldLayout,
  buildPoolSecondarySectionLayout,
  isOAuthPoolProviderType,
} from '@/features/pool/utils/poolAdvancedDialog'

describe('poolAdvancedDialog', () => {
  it('returns health toggle cards in the desktop display order', () => {
    expect(buildPoolHealthToggleCards().map(item => item.key)).toEqual([
      'health_policy_enabled',
      'probing_enabled',
      'auto_remove_banned_keys',
      'skip_exhausted_accounts',
      'pre_probe_enabled',
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
      {
        key: 'skip_exhausted_accounts',
        label: '跳过额度耗尽账号',
        description: '当 Codex / Kiro 账号额度已耗尽时，直接标记为不可调度并在请求侧跳过。',
      },
      {
        key: 'pre_probe_enabled',
        label: '候选预热',
        description: POOL_PRE_PROBE_HELP_TEXT,
      },
    ])
  })

  it('identifies OAuth pool providers for candidate preheat', () => {
    expect(['codex', 'kiro', 'antigravity', 'chatgpt_web'].every(isOAuthPoolProviderType)).toBe(true)
    expect(isOAuthPoolProviderType(' Codex ')).toBe(true)
    expect(isOAuthPoolProviderType('custom')).toBe(false)
    expect(POOL_PRE_PROBE_DISABLED_TOOLTIP).toBe('仅支持 OAuth 号池（Codex/Kiro/Antigravity/ChatGPT Web）')
  })

  it('returns all T13 pre-probe fields in a compact desktop layout', () => {
    expect(buildPoolPreProbeFieldLayout().fields.map(field => field.key)).toEqual([
      'top_n',
      'required_healthy',
      'dedup_window_secs',
      'cache_ttl_seconds',
      'cache_max_entries',
      'probe_timeout_seconds',
      'per_provider_rate_limit_per_minute',
      'group_lock_ttl_seconds',
      'circuit_failure_rate_threshold',
      'circuit_sample_window_seconds',
      'circuit_suspend_seconds',
      '5xx_streak_threshold',
    ])
    expect(buildPoolPreProbeFieldLayout().desktopColumnsClass).toBe('xl:grid-cols-3')
  })

  it('hydrates pre-probe form defaults and preserves custom subfields in payload', () => {
    const form = buildPoolPreProbeForm({
      enabled: true,
      top_n: 12,
      required_healthy: 6,
      '5xx_streak_threshold': 4,
    })

    expect(form.top_n).toBe(12)
    expect(form.required_healthy).toBe(6)
    expect(form.dedup_window_secs).toBe(300)

    const payload = buildPoolPreProbePayload(true, form)
    expect(payload).toMatchObject({
      enabled: true,
      top_n: 12,
      required_healthy: 6,
      dedup_window_secs: 300,
      '5xx_streak_threshold': 4,
    })
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
