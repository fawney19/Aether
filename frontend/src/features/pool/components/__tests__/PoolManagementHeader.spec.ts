import { describe, expect, it } from 'vitest'
import { createApp, defineComponent, h, nextTick } from 'vue'

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
  it('keeps page actions wired through component events', async () => {
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
          poolSchedulingLabel: '2 维度',
          showAdaptiveHotPoolMetricsButton: true,
          selectedCount: 2,
          isAllFilteredSelected: false,
          selectionDisabled: false,
          batchActionsDisabled: false,
          refreshLoading: false,
          refreshTitle: '刷新',
          onViewProvider: () => events.push('viewProvider'),
          onScheduling: () => events.push('scheduling'),
          onDemandMetrics: () => events.push('demandMetrics'),
          onAdvanced: () => events.push('advanced'),
          onToggleSelectAll: () => events.push('toggleSelectAll'),
          onBatchAction: (action: string) => events.push(`batchAction:${action}`),
          onRefresh: () => events.push('refresh'),
        })
      },
    })

    const root = document.createElement('div')
    document.body.appendChild(root)
    const app = createApp(Probe)
    app.use(createI18n())
    app.mount(root)

    root.querySelector<HTMLButtonElement>('[title="查看详情"]')?.click()
    root.querySelector<HTMLButtonElement>('[title="点击调整号池调度"]')?.click()
    root.querySelector<HTMLButtonElement>('[title="查看自适应热池指标"]')?.click()
    const desktopActions = root.querySelector('[data-testid="pool-header-actions"]')
    const advancedButton = desktopActions?.querySelector<HTMLButtonElement>('[title="高级设置"]')
    const selectAllButton = desktopActions?.querySelector<HTMLButtonElement>('[data-testid="pool-select-all-desktop"]')
    const batchActionsButton = desktopActions?.querySelector<HTMLButtonElement>('[data-testid="pool-batch-actions-desktop"]')
    advancedButton?.click()
    selectAllButton?.click()
    batchActionsButton?.click()
    await nextTick()
    document.body.querySelector<HTMLElement>('[data-testid="pool-batch-action-refresh_quota-desktop"]')?.click()
    await nextTick()
    root.querySelector<HTMLButtonElement>('[title="刷新"]')?.click()

    expect(events).toEqual([
      'viewProvider',
      'scheduling',
      'demandMetrics',
      'advanced',
      'toggleSelectAll',
      'batchAction:refresh_quota',
      'refresh',
    ])
    expect(selectAllButton?.textContent?.trim()).toBe('')
    expect(selectAllButton?.getAttribute('title')).toBe('全选')
    expect(advancedButton?.nextElementSibling).toBe(selectAllButton)
    expect(selectAllButton?.nextElementSibling).toBe(batchActionsButton)
    expect(batchActionsButton?.getAttribute('title')).toBe('选择执行动作')
    expect(root.querySelector('[title="账号批量操作"]')).toBeNull()
    expect(root.querySelector('[title="添加账号"]')).toBeNull()
    expect(root.querySelector('[title="提供商代理（未设置）"]')).toBeNull()
    expect(root.querySelector('[title="编辑端点"]')).toBeNull()
    expect(root.querySelector('[title="编辑提供商"]')).toBeNull()
    expect(root.querySelector('[title="当前状态：已启用，点击禁用提供商"]')).toBeNull()
    expect(root.textContent).toContain('2 维度')
    expect(root.querySelector('[data-testid="pool-selected-count-desktop"]')).toBeNull()
    expect(root.querySelector('[data-testid="pool-selected-count-mobile"]')?.textContent).toContain('已选 2 个')

    app.unmount()
    root.remove()
  })
})
