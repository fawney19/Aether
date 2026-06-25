import { describe, expect, it } from 'vitest'

import {
  buildPoolStatsDisplay,
  type PoolStatsKeyInput,
} from '@/features/pool/utils/poolStatsDisplay'

function metricValues(metrics: Array<{ key: string, value: string }>) {
  return Object.fromEntries(metrics.map(metric => [metric.key, metric.value]))
}

function createCodexKey(overrides: Partial<PoolStatsKeyInput> = {}): PoolStatsKeyInput {
  return {
    request_count: 1234,
    total_tokens: 5678000,
    total_cost_usd: '12.3456',
    status_snapshot: {
      quota: {
        windows: [
          {
            code: '5h',
            usage: {
              request_count: 5,
              total_tokens: 2500,
              total_cost_usd: '0.0045',
            },
          },
          {
            code: 'weekly',
            usage: {
              request_count: 0,
              total_tokens: 0,
              total_cost_usd: '0.00000000',
            },
          },
        ],
      },
    },
    ...overrides,
  }
}

describe('poolStatsDisplay', () => {
  it('builds Codex current-cycle groups in 5H and weekly order', () => {
    const display = buildPoolStatsDisplay(createCodexKey(), 'codex', 'current_cycle')

    expect(display.kind).toBe('codex_cycle')
    if (display.kind !== 'codex_cycle') throw new Error('expected codex cycle display')

    expect(display.groups.map(group => group.label)).toEqual(['5H', '周'])
    expect(metricValues(display.groups[0].metrics)).toEqual({
      request_count: '5',
      total_tokens: '2.5K',
      total_cost_usd: '$0.0045',
    })
    expect(metricValues(display.groups[1].metrics)).toEqual({
      request_count: '0',
      total_tokens: '0',
      total_cost_usd: '0',
    })
  })

  it('renders missing cycle usage as dashes instead of account-total fallback', () => {
    const display = buildPoolStatsDisplay(
      createCodexKey({
        status_snapshot: {
          quota: {
            windows: [{ code: '5h', usage: null }],
          },
        },
      }),
      'codex',
      'current_cycle',
    )

    expect(display.kind).toBe('codex_cycle')
    if (display.kind !== 'codex_cycle') throw new Error('expected codex cycle display')

    expect(metricValues(display.groups[0].metrics)).toEqual({
      request_count: '—',
      total_tokens: '—',
      total_cost_usd: '—',
    })
    expect(metricValues(display.groups[1].metrics)).toEqual({
      request_count: '—',
      total_tokens: '—',
      total_cost_usd: '—',
    })
  })

  it('preserves account-total formatting when toggled away from current cycle', () => {
    const display = buildPoolStatsDisplay(createCodexKey(), 'codex', 'account_total')

    expect(display.kind).toBe('account_total')
    if (display.kind !== 'account_total') throw new Error('expected account total display')

    expect(metricValues(display.metrics)).toEqual({
      request_count: '1,234',
      total_tokens: '5.7M',
      total_cost_usd: '$12.35',
    })
  })

  it('keeps non-Codex providers on account totals even in current-cycle mode', () => {
    const display = buildPoolStatsDisplay(createCodexKey(), 'openai', 'current_cycle')

    expect(display.kind).toBe('account_total')
    if (display.kind !== 'account_total') throw new Error('expected account total display')
    expect(metricValues(display.metrics)).toMatchObject({
      request_count: '1,234',
      total_tokens: '5.7M',
      total_cost_usd: '$12.35',
    })
  })

  it('builds GLM Coding Plan stats from official usage windows regardless of mode', () => {
    const display = buildPoolStatsDisplay(
      {
        request_count: 9999,
        total_tokens: 999999999,
        total_cost_usd: '999.99',
        status_snapshot: {
          quota: {
            windows: [
              {
                code: 'tokens_5h',
                usage: {
                  request_count: 90,
                  total_tokens: 3718682,
                },
              },
              {
                code: 'tokens_weekly',
                usage: {
                  request_count: 7357,
                  total_tokens: 490408764,
                },
              },
            ],
          },
        },
      },
      'glm_coding_plan',
      'account_total',
    )

    expect(display.kind).toBe('codex_cycle')
    if (display.kind !== 'codex_cycle') throw new Error('expected GLM official quota display')

    expect(display.groups.map(group => group.label)).toEqual(['5h', '周'])
    expect(metricValues(display.groups[0].metrics)).toEqual({
      request_count: '90',
      total_tokens: '3.7M',
    })
    expect(metricValues(display.groups[1].metrics)).toEqual({
      request_count: '7,357',
      total_tokens: '490.4M',
    })
  })

  it('leaves missing GLM weekly official usage blank instead of using account totals', () => {
    const display = buildPoolStatsDisplay(
      {
        request_count: 9999,
        total_tokens: 999999999,
        total_cost_usd: '999.99',
        status_snapshot: {
          quota: {
            windows: [
              {
                code: 'tokens_5h',
                usage: {
                  request_count: 90,
                  total_tokens: 3718682,
                },
              },
              {
                code: 'tokens_weekly',
                usage: null,
              },
            ],
          },
        },
      },
      'glm_coding_plan',
      'account_total',
    )

    expect(display.kind).toBe('codex_cycle')
    if (display.kind !== 'codex_cycle') throw new Error('expected GLM official usage display')

    expect(metricValues(display.groups[0].metrics)).toEqual({
      request_count: '90',
      total_tokens: '3.7M',
    })
    expect(metricValues(display.groups[1].metrics)).toEqual({
      request_count: '—',
      total_tokens: '—',
    })
  })

  it('promotes large token totals above M', () => {
    const display = buildPoolStatsDisplay(
      createCodexKey({ total_tokens: 1_500_000_000 }),
      'openai',
      'account_total',
    )

    expect(display.kind).toBe('account_total')
    if (display.kind !== 'account_total') throw new Error('expected account total display')
    expect(metricValues(display.metrics).total_tokens).toBe('1.5B')
  })
})
