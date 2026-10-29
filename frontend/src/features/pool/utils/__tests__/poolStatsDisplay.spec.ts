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
            window_minutes: 300,
            usage: {
              request_count: 5,
              total_tokens: 2500,
              total_cost_usd: '0.0045',
            },
          },
          {
            code: 'weekly',
            window_minutes: 10_080,
            usage: {
              request_count: 8,
              total_tokens: 5000,
              total_cost_usd: '0.012',
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
      request_count: '8',
      total_tokens: '5K',
      total_cost_usd: '$0.012',
    })
  })

  it('renders missing cycle usage as dashes instead of account-total fallback', () => {
    const display = buildPoolStatsDisplay(
      createCodexKey({
        status_snapshot: {
          quota: {
            windows: [{ code: '5h', window_minutes: 300, usage: null }],
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
    expect(display.groups).toHaveLength(1)
  })

  it('builds monthly stats from the actual quota window and ignores zero placeholders', () => {
    const display = buildPoolStatsDisplay(
      createCodexKey({
        status_snapshot: {
          quota: {
            windows: [
              {
                code: 'monthly',
                label: '月',
                window_minutes: 43_800,
                usage: {
                  request_count: 12,
                  total_tokens: 3456,
                  total_cost_usd: '0.125',
                },
              },
              {
                code: 'weekly',
                label: '周',
                window_minutes: 0,
                usage: {
                  request_count: 99,
                },
              },
            ],
          },
        },
      }),
      'codex',
      'current_cycle',
    )

    expect(display.kind).toBe('codex_cycle')
    if (display.kind !== 'codex_cycle') throw new Error('expected codex cycle display')
    expect(display.groups.map(group => group.label)).toEqual(['月'])
    expect(metricValues(display.groups[0].metrics)).toEqual({
      request_count: '12',
      total_tokens: '3.5K',
      total_cost_usd: '$0.125',
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

  it('builds xAI weekly cycle stats only from annotated usage windows', () => {
    const display = buildPoolStatsDisplay(
      {
        request_count: 1291,
        status_snapshot: {
          quota: {
            windows: [
              {
                code: 'usage',
                label: '周额度',
                scope: 'account',
                window_minutes: 10_080,
                usage: { request_count: 12, total_tokens: 3000, total_cost_usd: '0.5' },
              },
              { code: 'prepaid', label: '预付额度', scope: 'account' },
            ],
          },
        },
      },
      'xai',
      'current_cycle',
    )

    expect(display.kind).toBe('codex_cycle')
    if (display.kind !== 'codex_cycle') throw new Error('expected cycle display')
    expect(display.groups.map(group => group.label)).toEqual(['周'])
    expect(metricValues(display.groups[0].metrics).request_count).toBe('12')
  })

  it('groups Antigravity quota-group cycle stats by model family', () => {
    const groupWindow = (index: number, bucketId: string, minutes: number, requests: number) => ({
      code: `group:${index}:${bucketId}`,
      scope: 'quota_group',
      quota_group_label: index === 0 ? 'Gemini Models' : 'Claude and GPT models',
      bucket_id: bucketId,
      window_minutes: minutes,
      usage: { request_count: requests, total_tokens: 0, total_cost_usd: '0' },
    })
    const display = buildPoolStatsDisplay(
      {
        status_snapshot: {
          quota: {
            windows: [
              groupWindow(1, '3p-weekly', 10_080, 4),
              groupWindow(0, 'gemini-weekly', 10_080, 30),
              { code: 'model:gemini-3-flash', scope: 'model', label: 'Gemini 3 Flash' },
              groupWindow(0, 'gemini-5h', 300, 3),
              groupWindow(1, '3p-5h', 300, 1),
            ],
          },
        },
      },
      'antigravity',
      'current_cycle',
    )

    expect(display.kind).toBe('codex_cycle')
    if (display.kind !== 'codex_cycle') throw new Error('expected cycle display')
    expect(display.groups.map(group => [group.section, group.label])).toEqual([
      ['Gemini', 'Gemini 5H'],
      ['Gemini', 'Gemini 周'],
      ['Claude/GPT', 'Claude/GPT 5H'],
      ['Claude/GPT', 'Claude/GPT 周'],
    ])
    expect(display.groups.map(group => metricValues(group.metrics).request_count))
      .toEqual(['3', '30', '1', '4'])
  })

  it('keeps account totals for non-codex keys without cycle windows', () => {
    const display = buildPoolStatsDisplay(
      { request_count: 7, status_snapshot: { quota: { windows: [] } } },
      'antigravity',
      'current_cycle',
    )

    expect(display.kind).toBe('account_total')
  })
})
