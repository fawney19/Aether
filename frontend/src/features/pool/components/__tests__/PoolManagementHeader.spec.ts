import { describe, expect, it } from 'vitest'
import { createApp, defineComponent, h } from 'vue'

import PoolManagementHeader from '@/features/pool/components/PoolManagementHeader.vue'
import { createI18n } from '@/i18n'
import type { PoolOverviewItem } from '@/api/endpoints/pool'

const provider = {
  provider_id: 'provider-1',
  provider_name: 'Codex Provider',
  provider_type: 'codex',
  pool_enabled: true,
  total_keys: 2,
} as PoolOverviewItem

describe('PoolManagementHeader', () => {
  it('keeps page actions wired through component events', () => {
    const events: string[] = []
    const Probe = defineComponent({
      setup() {
        return () => h(PoolManagementHeader, {
          providers: [provider],
          providerId: 'provider-1',
          providerSelectDisabled: false,
          status: 'all',
          statusOptions: [{ value: 'all', label: '全部状态' }],
          search: '',
          metaText: 'codex | 启用',
          providerProxyNodeId: null,
          providerProxyMobileOpen: false,
          providerProxyDesktopOpen: false,
          providerProxyButtonTitle: '提供商代理（未设置）',
          savingProviderProxy: false,
          poolSchedulingLabel: '2 维度',
          showAdaptiveHotPoolMetricsButton: true,
          providerToggleButtonTitle: '当前状态：已启用，点击禁用提供商',
          togglingProviderStatus: false,
          refreshLoading: false,
          refreshTitle: '刷新',
          onImport: () => events.push('import'),
          onScheduling: () => events.push('scheduling'),
          onDemandMetrics: () => events.push('demandMetrics'),
          onRefresh: () => events.push('refresh'),
        })
      },
    })

    const root = document.createElement('div')
    document.body.appendChild(root)
    const app = createApp(Probe)
    app.use(createI18n())
    app.mount(root)

    root.querySelector<HTMLButtonElement>('[title="添加账号"]')?.click()
    root.querySelector<HTMLButtonElement>('[title="点击调整号池调度"]')?.click()
    root.querySelector<HTMLButtonElement>('[title="查看自适应热池指标"]')?.click()
    root.querySelector<HTMLButtonElement>('[title="刷新"]')?.click()

    expect(events).toEqual(['import', 'scheduling', 'demandMetrics', 'refresh'])
    expect(root.textContent).toContain('2 维度')

    app.unmount()
    root.remove()
  })
})
