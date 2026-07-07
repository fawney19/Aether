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
      cycleRows: [{
        key: 'request_count',
        label: '请求',
        fiveH: { key: 'request_count', label: '请求', value: '12', missing: false },
        weekly: { key: 'request_count', label: '请求', value: '88', missing: false },
      }],
      accountMetrics: [],
    })
    app.use(createI18n())
    app.mount(root)

    expect(root.querySelector('[data-testid="pool-stats-cycle-grid"]')).toBeTruthy()
    expect(root.querySelector('[data-testid="pool-stats-5h-request_count"]')?.textContent).toBe('12')
    expect(root.querySelector('[data-testid="pool-stats-weekly-request_count"]')?.textContent).toBe('88')

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
    expect(root.textContent).toContain('额度未知')

    app.unmount()
    root.remove()
  })
})
