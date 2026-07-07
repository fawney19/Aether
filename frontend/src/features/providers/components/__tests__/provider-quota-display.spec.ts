import { describe, expect, it } from 'vitest'
import { createApp, defineComponent, h } from 'vue'

import ProviderMonthlyQuotaCard from '@/features/providers/components/ProviderMonthlyQuotaCard.vue'
import ProviderQuotaProgressRow from '@/features/providers/components/ProviderQuotaProgressRow.vue'
import ProviderQuotaSectionHeader from '@/features/providers/components/ProviderQuotaSectionHeader.vue'
import { createI18n } from '@/i18n'

function mount(component: Parameters<typeof createApp>[0], props?: Record<string, unknown>) {
  const root = document.createElement('div')
  document.body.appendChild(root)
  const app = createApp(component, props)
  app.use(createI18n())
  app.mount(root)

  return {
    root,
    unmount: () => {
      app.unmount()
      root.remove()
    },
  }
}

describe('provider quota display components', () => {
  it('renders monthly quota usage and reset day', () => {
    const { root, unmount } = mount(ProviderMonthlyQuotaCard, {
      used: 25,
      quota: 100,
      resetDay: 15,
    })

    expect(root.querySelector('[data-testid="provider-monthly-quota-card"]')).toBeTruthy()
    expect(root.querySelector('[data-testid="provider-monthly-quota-percent"]')?.textContent).toContain('25.0%')
    expect(root.querySelector('[data-testid="provider-monthly-quota-amount"]')?.textContent).toContain('$25.00 / $100.00')
    expect(root.querySelector('[data-testid="provider-monthly-quota-reset"]')?.textContent).toContain('15')

    unmount()
  })

  it('normalizes quota progress and renders fallback footer text', () => {
    const { root, unmount } = mount(ProviderQuotaProgressRow, {
      label: 'Daily',
      remainingPercent: 120,
      meterClass: 'text-green-600',
      barClass: 'bg-green-500',
      resetText: '2h reset',
    })

    expect(root.querySelector('[data-testid="provider-quota-progress-meter"]')?.textContent?.trim()).toBe('100.0%')
    expect((root.querySelector('[data-testid="provider-quota-progress-bar"]') as HTMLElement).style.width).toBe('100%')
    expect(root.querySelector('[data-testid="provider-quota-progress-reset"]')?.textContent).toBe('2h reset')

    unmount()
  })

  it('renders section loading and updated state', () => {
    const Probe = defineComponent({
      setup() {
        return () => h(ProviderQuotaSectionHeader, {
          title: 'Account quota',
          loading: true,
          updatedText: '10:30',
        })
      },
    })

    const { root, unmount } = mount(Probe)

    expect(root.textContent).toContain('Account quota')
    expect(root.querySelector('[data-testid="provider-quota-header-loading"]')).toBeTruthy()
    expect(root.querySelector('[data-testid="provider-quota-header-updated"]')?.textContent).toBe('10:30')

    unmount()
  })
})
