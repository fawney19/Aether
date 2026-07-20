import { describe, expect, it } from 'vitest'
import { createApp, defineComponent, h } from 'vue'

import PoolKeyQuotaPanel from '@/features/pool/components/PoolKeyQuotaPanel.vue'
import PoolKeyStatsPanel from '@/features/pool/components/PoolKeyStatsPanel.vue'
import { createI18n } from '@/i18n'

describe('pool key display panels', () => {
  it('renders codex cycle stats with stable test hooks', () => {
    const root = document.createElement('div')
    document.body.appendChild(root)
    const app = createApp(PoolKeyStatsPanel, {
      cycle: true,
      cycleGroups: [
        {
          code: '5h',
          label: '5H',
          metrics: [{ key: 'request_count', label: '请求', value: '12', missing: false, numericValue: 12 }],
        },
        {
          code: 'weekly',
          label: '周',
          metrics: [{ key: 'request_count', label: '请求', value: '88', missing: false, numericValue: 88 }],
        },
      ],
      accountMetrics: [],
    })
    app.use(createI18n())
    app.mount(root)

    const stats = root.querySelector('[data-testid="pool-stats-cycle-text"]')
    const requestValue = root.querySelector('[data-testid="pool-stats-cycle-request_count"]')
    expect(stats).toBeTruthy()
    expect(stats?.className).toContain('w-full')
    expect(stats?.className).toContain('max-w-[168px]')
    expect(requestValue?.textContent?.trim()).toBe('12/88')
    expect(requestValue?.previousElementSibling?.textContent?.trim()).toBe('请求')
    expect(requestValue?.parentElement?.className).toContain('justify-between')
    expect(requestValue?.className).toContain('grid-cols-[minmax(0,1fr)_auto_minmax(0,1fr)]')
    expect(requestValue?.className).toContain('w-[112px]')
    expect(requestValue?.children[0]?.className).toContain('text-right')
    expect(requestValue?.children[1]?.textContent).toBe('/')
    expect(requestValue?.children[2]?.className).toContain('text-left')
    expect(root.querySelectorAll('[data-cycle-stat-part="divider"]')).toHaveLength(3)
    expect(root.querySelector('[data-testid="pool-stats-cycle-small-overlay"]')).toBeNull()
    expect(root.querySelector('[data-testid="pool-stats-cycle-large-base"]')).toBeNull()
    expect(root.textContent).not.toContain('5H')
    expect(root.textContent).not.toContain('周')

    app.unmount()
    root.remove()
  })

  it('renders quota progress rows and fallback quota text', () => {
    const Probe = defineComponent({
      setup() {
        return () => h('div', [
          h(PoolKeyQuotaPanel, {
            items: [{
              label: '5H',
              remainingPercent: 42,
              resetText: '1h 后重置',
              meterText: '42.0%',
              barClass: 'bg-amber-500',
              meterClass: 'text-amber-600',
            }],
            freshnessText: '额度数据已过期 · 更新 07-19 12:00',
            freshnessStale: true,
          }),
          h(PoolKeyQuotaPanel, {
            items: [],
            accountQuotaText: null,
            fallbackText: '额度未知',
            textClass: 'text-muted-foreground',
            variant: 'mobile',
          }),
        ])
      },
    })

    const root = document.createElement('div')
    document.body.appendChild(root)
    const app = createApp(Probe)
    app.use(createI18n())
    app.mount(root)

    expect(root.querySelector('[data-testid="pool-quota-reset-text"]')?.textContent).toBe('1h 后重置')
    expect(root.querySelector('[data-testid="pool-quota-meter-text"]')?.textContent).toBe('42.0%')
    expect(root.querySelector('[data-testid="pool-quota-freshness"]')?.textContent)
      .toBe('额度数据已过期 · 更新 07-19 12:00')
    expect(root.querySelector('[data-testid="pool-quota-freshness"]')?.className).toContain('text-amber-600')
    expect(root.textContent).toContain('额度未知')

    app.unmount()
    root.remove()
  })

  it('renders single-cycle stats as plain text', () => {
    const root = document.createElement('div')
    document.body.appendChild(root)
    const app = createApp(PoolKeyStatsPanel, {
      cycle: true,
      cycleGroups: [{
        code: 'monthly',
        label: '月',
        metrics: [
          { key: 'request_count', label: '请求', value: '31', missing: false, numericValue: 31 },
          { key: 'total_tokens', label: 'Token', value: '38.8K', missing: false, numericValue: 38_800 },
          { key: 'total_cost_usd', label: '费用', value: '$0.077', missing: false, numericValue: 0.077 },
        ],
      }],
      accountMetrics: [],
    })
    app.use(createI18n())
    app.mount(root)

    const requestValue = root.querySelector('[data-testid="pool-stats-cycle-request_count"]')
    expect(requestValue?.textContent?.trim()).toBe('-/31')
    expect(requestValue?.previousElementSibling?.textContent?.trim()).toBe('请求')
    expect(requestValue?.className).toContain('grid-cols-[minmax(0,1fr)_auto_minmax(0,1fr)]')
    expect(requestValue?.children[0]?.textContent).toBe('-')
    expect(requestValue?.children[1]?.textContent).toBe('/')
    expect(requestValue?.children[1]?.className).toContain('w-1.5')
    expect(requestValue?.children[2]?.textContent).toBe('31')
    expect(requestValue?.children[2]?.className).toContain('text-left')
    expect(root.querySelector('[data-testid="pool-stats-cycle-single-marker"]')).toBeNull()
    expect(root.querySelector('[data-testid="pool-stats-cycle-bar-request_count"]')).toBeNull()
    expect(root.textContent).not.toContain('月')

    app.unmount()
    root.remove()
  })
})
