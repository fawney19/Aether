import { describe, expect, it } from 'vitest'
import { createApp, defineComponent, h } from 'vue'

import ProviderMonthlyQuotaCard from '@/features/providers/components/ProviderMonthlyQuotaCard.vue'
import ProviderQuotaProgressRow from '@/features/providers/components/ProviderQuotaProgressRow.vue'
import ProviderQuotaSectionHeader from '@/features/providers/components/ProviderQuotaSectionHeader.vue'
import { createI18n, setI18nLocale } from '@/i18n'

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
  it('renders quota usage and the interval-day reset semantics', () => {
    setI18nLocale('zh-CN')
    const { root, unmount } = mount(ProviderMonthlyQuotaCard, {
      used: 25,
      quota: 100,
      resetIntervalDays: 15,
    })

    expect(root.querySelector('[data-testid="provider-monthly-quota-card"]')).toBeTruthy()
    expect(root.querySelector('[data-testid="provider-monthly-quota-percent"]')?.textContent).toContain('25.0%')
    expect(root.querySelector('[data-testid="provider-monthly-quota-amount"]')?.textContent).toContain('$25.00 / $100.00')
    expect(root.querySelector('[data-testid="provider-monthly-quota-reset"]')?.textContent?.trim()).toBe('每 15 天重置')
    expect(root.querySelector('[data-testid="provider-monthly-quota-reset"]')?.textContent).not.toContain('每月')

    unmount()
  })

  it('shows every Sub2API subscription window and marks the locally mapped one', () => {
    setI18nLocale('zh-CN')
    const { root, unmount } = mount(ProviderMonthlyQuotaCard, {
      resetIntervalDays: 1,
      remoteQuotaGroup: {
        group_id: '42',
        group_name: 'Pro',
        subscription_id: '9',
        daily_limit_usd: 10,
        daily_used_usd: 2,
        weekly_limit_usd: 50,
        weekly_used_usd: 8,
        monthly_limit_usd: 0,
        monthly_used_usd: 0,
        local_sync_window: 'daily',
        expires_at_unix_secs: null,
      },
    })

    expect(root.querySelector('[data-testid="provider-remote-quota-daily"]')?.textContent).toContain('日限额')
    expect(root.querySelector('[data-testid="provider-remote-quota-daily"]')?.textContent).toContain('$2 / $10')
    expect(root.querySelector('[data-testid="provider-remote-quota-daily"]')?.textContent).toContain('映射到本地')
    expect(root.querySelector('[data-testid="provider-remote-quota-weekly"]')?.textContent).toContain('$8 / $50')
    expect(root.querySelector('[data-testid="provider-remote-quota-monthly"]')?.textContent).toContain('不限')
    expect(root.querySelector('[data-testid="provider-remote-quota-local-window"]')?.textContent?.replace(/\s+/g, ' ').trim()).toBe('日限额 · 每日重置')
    expect(root.textContent).not.toContain('Group ID')

    unmount()
  })

  it('separates locally enforced, upstream-confirmed, and in-sync local usage', () => {
    setI18nLocale('zh-CN')
    const { root, unmount } = mount(ProviderMonthlyQuotaCard, {
      used: 2.5,
      quota: 10,
      billingType: 'monthly_quota',
      resetIntervalDays: 1,
      remoteQuotaEnabled: true,
      remoteQuotaGroup: {
        group_id: '42',
        group_name: 'Pro',
        subscription_id: '9',
        daily_limit_usd: 10,
        daily_used_usd: 2,
        weekly_limit_usd: 50,
        weekly_used_usd: 8,
        monthly_limit_usd: 0,
        monthly_used_usd: 0,
        local_sync_window: 'daily',
        expires_at_unix_secs: null,
        sync_status: 'applied',
        remote_confirmed_used_usd: 2.25,
      },
    })

    expect(root.querySelector('[data-testid="provider-remote-quota-local-effective"]')?.textContent).toContain('本地生效额度')
    expect(root.querySelector('[data-testid="provider-remote-quota-local-effective"]')?.textContent).toContain('$2.50 / $10.00')
    expect(root.querySelector('[data-testid="provider-remote-quota-confirmed-used"]')?.textContent).toContain('上游已确认: $2.25')
    expect(root.querySelector('[data-testid="provider-remote-quota-pending-local"]')?.textContent).toContain('同步期间本地增量: $0.25')
    expect(root.textContent).toContain('Sub2API 上游套餐窗口')

    unmount()
  })

  it('does not label a remote window active when the latest apply failed', () => {
    setI18nLocale('zh-CN')
    const { root, unmount } = mount(ProviderMonthlyQuotaCard, {
      used: 4,
      quota: 10,
      billingType: 'monthly_quota',
      remoteQuotaEnabled: true,
      resetIntervalDays: 1,
      remoteQuotaGroup: {
        group_id: '42',
        group_name: 'Pro',
        subscription_id: '9',
        daily_limit_usd: 10,
        daily_used_usd: 2,
        weekly_limit_usd: 50,
        weekly_used_usd: 8,
        monthly_limit_usd: 0,
        monthly_used_usd: 0,
        local_sync_window: 'daily',
        expires_at_unix_secs: null,
        sync_status: 'failed_keep_local',
        sync_message: 'progress 数据缺失',
      },
    })

    expect(root.querySelector('[data-testid="provider-remote-quota-daily"]')?.textContent).not.toContain('映射到本地')
    expect(root.querySelector('[data-testid="provider-remote-quota-local-effective"]')?.textContent).toContain('$4.00 / $10.00')
    expect(root.querySelector('[data-testid="provider-remote-quota-sync-warning"]')?.textContent).toContain('progress 数据缺失')
    expect(root.querySelector('[data-testid="provider-remote-quota-local-window"]')).toBeNull()

    unmount()
  })

  it('shows an exhausted remote subscription without an active Group snapshot', () => {
    setI18nLocale('zh-CN')
    const { root, unmount } = mount(ProviderMonthlyQuotaCard, {
      billingType: 'monthly_quota',
      quota: 0,
      remoteQuotaEnabled: true,
    })

    expect(root.querySelector('[data-testid="provider-monthly-quota-card"]')).toBeTruthy()
    expect(root.querySelector('[data-testid="provider-remote-quota-fallback"]')?.textContent).toContain('额度已用尽')

    unmount()
  })

  it('shows an all-unlimited remote subscription even without a local quota amount', () => {
    setI18nLocale('zh-CN')
    const { root, unmount } = mount(ProviderMonthlyQuotaCard, {
      quota: 0,
      remoteQuotaGroup: {
        group_id: '42',
        group_name: 'Unlimited',
        subscription_id: '9',
        daily_limit_usd: 0,
        daily_used_usd: 0,
        weekly_limit_usd: 0,
        weekly_used_usd: 0,
        monthly_limit_usd: 0,
        monthly_used_usd: 0,
        local_sync_window: null,
        expires_at_unix_secs: null,
      },
    })

    expect(root.querySelector('[data-testid="provider-monthly-quota-card"]')).toBeTruthy()
    expect(root.querySelector('[data-testid="provider-remote-quota-daily"]')?.textContent).toContain('不限')
    expect(root.querySelector('[data-testid="provider-remote-quota-weekly"]')?.textContent).toContain('不限')
    expect(root.querySelector('[data-testid="provider-remote-quota-monthly"]')?.textContent).toContain('不限')
    expect(root.querySelector('[data-testid="provider-remote-quota-local-window"]')?.textContent).toContain('日、周、月均不限')
    expect(root.textContent).not.toContain('映射到本地')

    unmount()
  })

  it('labels a one-day reset interval as daily', () => {
    setI18nLocale('zh-CN')
    const { root, unmount } = mount(ProviderMonthlyQuotaCard, {
      used: 2,
      quota: 10,
      resetIntervalDays: 1,
    })

    expect(root.querySelector('[data-testid="provider-monthly-quota-reset"]')?.textContent?.trim()).toBe('每日重置')

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
