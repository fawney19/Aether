import { describe, expect, it } from 'vitest'

import {
  getGeminiCliAccountCreditsText,
  getQuotaDisplayText,
} from '../providerKeyQuota'

describe('providerKeyQuota', () => {
  it('includes Codex Spark quota windows in display text', () => {
    expect(getQuotaDisplayText({
      status_snapshot: {
        oauth: {
          code: 'valid',
        },
        account: {
          code: 'ok',
          blocked: false,
        },
        quota: {
          provider_type: 'codex',
          code: 'ok',
          exhausted: false,
          windows: [
            {
              code: 'weekly',
              remaining_ratio: 0.9,
            },
            {
              code: '5h',
              remaining_ratio: 0.8,
            },
            {
              code: 'spark_5h',
              remaining_ratio: 0.6,
            },
            {
              code: 'spark_weekly',
              remaining_ratio: 0.95,
            },
          ],
        },
      },
    }, 'codex')).toBe('周剩余 90.0% | 5H剩余 80.0% | Spark5H剩余 60.0% | Spark周剩余 95.0%')
  })

  it('formats Grok account quota from structured quota windows', () => {
    expect(getQuotaDisplayText({
      status_snapshot: {
        oauth: {
          code: 'valid',
        },
        account: {
          code: 'ok',
          blocked: false,
        },
        quota: {
          provider_type: 'grok',
          code: 'ok',
          exhausted: false,
          windows: [
            {
              scope: 'account',
              used_value: 2,
              limit_value: 10,
              remaining_ratio: 0.8,
            },
          ],
        },
      },
    }, 'grok')).toBe('剩余 80.0% (8/10)')
  })

  it('formats Grok mode quota from model-scoped windows', () => {
    expect(getQuotaDisplayText({
      status_snapshot: {
        oauth: {
          code: 'valid',
        },
        account: {
          code: 'ok',
          blocked: false,
        },
        quota: {
          provider_type: 'grok',
          code: 'ok',
          exhausted: false,
          plan_type: 'heavy',
          windows: [
            {
              code: 'model:quota_auto',
              label: 'auto',
              scope: 'model',
              remaining_ratio: 0.4,
              used_value: 90,
              limit_value: 150,
            },
            {
              code: 'model:quota_heavy',
              label: 'heavy',
              scope: 'model',
              remaining_ratio: 0,
              used_value: 20,
              limit_value: 20,
            },
          ],
        },
      },
    }, 'grok')).toBe('Auto剩余 40.0% (60/150) | Heavy剩余 0.0% (0/20)')
  })

  it('formats Gemini CLI AI credits from status snapshot and upstream metadata', () => {
    expect(getQuotaDisplayText({
      status_snapshot: {
        quota: {
          provider_type: 'gemini_cli',
          code: 'ok',
          exhausted: false,
          credits: {
            remaining: 123.5,
            consumed: 7,
          },
        },
      },
    }, 'gemini_cli')).toBe('AI Credits 剩余 123.5')

    expect(getGeminiCliAccountCreditsText({
      status_snapshot: {
        quota: {
          provider_type: 'gemini_cli',
          code: 'ok',
          exhausted: false,
        },
      },
      upstream_metadata: {
        gemini_cli: {
          paidTier: {
            id: 'g1-pro-tier',
            availableCredits: '41.5',
          },
        },
      },
    }, 'gemini_cli')).toBe('AI Credits 剩余 41.5')
  })

  it('formats ChatGPT Web image quota as remaining count', () => {
    expect(getQuotaDisplayText({
      status_snapshot: {
        quota: {
          provider_type: 'chatgpt_web',
          code: 'ok',
          exhausted: false,
          windows: [
            {
              code: 'image_gen',
              scope: 'account',
              remaining_ratio: 0.96,
              used_value: 1,
              remaining_value: 24,
              limit_value: 25,
            },
          ],
        },
      },
    }, 'chatgpt_web')).toBe('生图剩余 24/25')
  })

  it('surfaces Windsurf hard account states', () => {
    expect(getQuotaDisplayText({
      status_snapshot: {
        quota: {
          provider_type: 'windsurf',
          code: 'quarantined',
          label: '账号隔离中',
          exhausted: false,
        },
      },
    }, 'windsurf')).toBe('账号隔离中')

    expect(getQuotaDisplayText({
      status_snapshot: {
        quota: {
          provider_type: 'windsurf',
          code: 'cooldown',
          label: '冷却中',
          exhausted: false,
        },
      },
    }, 'windsurf')).toBe('冷却中')

    expect(getQuotaDisplayText({
      status_snapshot: {
        quota: {
          provider_type: 'windsurf',
          code: 'cooldown',
          exhausted: false,
        },
      },
    }, 'windsurf')).toBe('冷却中')
  })

  it('includes Windsurf quota windows and model availability in display text', () => {
    expect(getQuotaDisplayText({
      status_snapshot: {
        quota: {
          provider_type: 'windsurf',
          code: 'ok',
          exhausted: false,
          allowed_models_count: 7,
          windows: [
            {
              code: 'daily',
              remaining_ratio: 0.75,
            },
            {
              code: 'weekly',
              remaining_ratio: 0.5,
            },
            {
              code: 'prompt',
              remaining_value: 12,
              limit_value: 20,
            },
            {
              code: 'flex',
              used_value: 2,
              limit_value: 5,
            },
          ],
        },
      },
    }, 'windsurf')).toBe('日剩余 75.0% | 周剩余 50.0% | Prompt 剩余 12/20 | Flex 剩余 3/5 | 可用模型 7 个')

    expect(getQuotaDisplayText({
      status_snapshot: {
        quota: {
          provider_type: 'windsurf',
          code: 'cooldown',
          label: '冷却中',
          exhausted: false,
          rate_limit: {
            limited: true,
            has_capacity: true,
            messages_remaining: -1,
            max_messages: -1,
          },
          allowed_models_count: 118,
          windows: [
            {
              code: 'daily',
              remaining_ratio: 0.99,
            },
            {
              code: 'weekly',
              remaining_ratio: 1,
            },
            {
              code: 'prompt',
              remaining_value: 100,
              limit_value: 100,
            },
            {
              code: 'rate_limit',
              reset_seconds: null,
              is_exhausted: false,
            },
          ],
        },
      },
    }, 'windsurf')).toBe('日剩余 99.0% | 周剩余 100.0% | Prompt 剩余 100/100 | 可用模型 118 个')
  })

  it('uses Windsurf model availability when no quota window is present', () => {
    expect(getQuotaDisplayText({
      status_snapshot: {
        quota: {
          provider_type: 'windsurf',
          code: 'ok',
          exhausted: false,
          allowed_models_count: 3,
        },
      },
    }, 'windsurf')).toBe('可用模型 3 个')
  })
})
