import { describe, expect, it, vi } from 'vitest'
import { createApp, defineComponent, h } from 'vue'

import ProviderKeyActionCluster from '@/features/providers/components/ProviderKeyActionCluster.vue'
import type { EndpointAPIKey } from '@/api/endpoints'
import { createI18n } from '@/i18n'

vi.mock('@/components/ui', async () => {
  const { defineComponent, h } = await import('vue')

  const passthrough = (name: string) => defineComponent({
    name,
    setup(_, { slots }) {
      return () => h('div', slots.default?.())
    },
  })

  return {
    Popover: passthrough('PopoverStub'),
    PopoverTrigger: passthrough('PopoverTriggerStub'),
    PopoverContent: passthrough('PopoverContentStub'),
  }
})

vi.mock('@/features/providers/components/ProxyNodeSelect.vue', async () => {
  const { defineComponent, h } = await import('vue')

  return {
    default: defineComponent({
      name: 'ProxyNodeSelectStub',
      emits: ['update:modelValue'],
      setup(_, { emit }) {
        return () => h('button', {
          type: 'button',
          'data-testid': 'proxy-node-select',
          onClick: () => emit('update:modelValue', 'proxy-node-2'),
        }, 'select proxy')
      },
    }),
  }
})

vi.mock('lucide-vue-next', async () => {
  const { defineComponent, h } = await import('vue')
  const Icon = defineComponent({
    name: 'IconStub',
    setup() {
      return () => h('span')
    },
  })

  return {
    BarChart3: Icon,
    Edit: Icon,
    Globe: Icon,
    Power: Icon,
    RefreshCw: Icon,
    Shield: Icon,
    Trash2: Icon,
  }
})

function createProviderKey(overrides: Partial<EndpointAPIKey> = {}): EndpointAPIKey {
  return {
    id: 'provider-key-1',
    provider_id: 'provider-1',
    api_formats: ['openai:chat'],
    api_key_masked: 'sk-***',
    auth_type: 'api_key',
    name: 'Primary key',
    internal_priority: 10,
    cache_ttl_minutes: 0,
    max_probe_interval_minutes: 5,
    health_score: 0.42,
    consecutive_failures: 0,
    request_count: 0,
    success_count: 0,
    error_count: 0,
    success_rate: 1,
    avg_response_time_ms: 0,
    is_active: true,
    created_at: '2026-01-01T00:00:00Z',
    updated_at: '2026-01-01T00:00:00Z',
    ...overrides,
  }
}

function mount(props: Record<string, unknown>) {
  const root = document.createElement('div')
  document.body.appendChild(root)
  const app = createApp(defineComponent({
    setup() {
      return () => h(ProviderKeyActionCluster, props)
    },
  }))
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

describe('ProviderKeyActionCluster', () => {
  it('renders circuit, health, proxy and antigravity actions', () => {
    const { root, unmount } = mount({
      apiKey: createProviderKey({
        circuit_breaker_open: true,
        proxy: { node_id: 'proxy-node-1' },
      }),
      providerType: 'antigravity',
      recoverable: true,
      recoverTitle: 'Recover key',
      circuitBreakerTitle: 'Circuit is open',
      circuitProbeCountdown: ' 2m',
      healthScoreBarClass: 'bg-red-500',
      healthScoreTextClass: 'text-red-600',
      proxyPopoverOpen: true,
      proxyNodeName: 'Tokyo',
    })

    expect(root.querySelector('[data-testid="provider-key-circuit-badge"]')?.textContent).toContain('熔断 2m')
    expect(root.querySelector('[data-testid="provider-key-health"]')?.textContent).toContain('42%')
    expect(root.querySelector('button[title="Recover key"]')).toBeTruthy()
    expect(root.querySelector('button[title="代理: Tokyo"]')).toBeTruthy()
    expect(root.querySelector('button[title="配额详情"]')).toBeTruthy()

    unmount()
  })

  it('emits operation and proxy events without owning business logic', () => {
    const onRecover = vi.fn()
    const onPermissions = vi.fn()
    const onEdit = vi.fn()
    const onOpenAntigravityQuota = vi.fn()
    const onToggleActive = vi.fn()
    const onDelete = vi.fn()
    const onClearProxy = vi.fn()
    const onSetProxy = vi.fn()

    const { root, unmount } = mount({
      apiKey: createProviderKey({ proxy: { node_id: 'proxy-node-1' } }),
      providerType: 'antigravity',
      recoverable: true,
      recoverTitle: 'Recover key',
      proxyPopoverOpen: true,
      proxyNodeName: 'Tokyo',
      onRecover,
      onPermissions,
      onEdit,
      onOpenAntigravityQuota,
      onToggleActive,
      onDelete,
      onClearProxy,
      onSetProxy,
    })

    ;(root.querySelector('button[title="Recover key"]') as HTMLButtonElement).click()
    ;(root.querySelector('button[title="模型权限"]') as HTMLButtonElement).click()
    ;(root.querySelector('button[title="编辑密钥"]') as HTMLButtonElement).click()
    ;(root.querySelector('button[title="配额详情"]') as HTMLButtonElement).click()
    ;(root.querySelector('button[title="点击停用"]') as HTMLButtonElement).click()
    ;(root.querySelector('button[title="删除密钥"]') as HTMLButtonElement).click()
    ;(Array.from(root.querySelectorAll('button')).find(button => button.textContent?.includes('清除')) as HTMLButtonElement).click()
    ;(root.querySelector('[data-testid="proxy-node-select"]') as HTMLButtonElement).click()

    expect(onRecover).toHaveBeenCalledTimes(1)
    expect(onPermissions).toHaveBeenCalledTimes(1)
    expect(onEdit).toHaveBeenCalledTimes(1)
    expect(onOpenAntigravityQuota).toHaveBeenCalledTimes(1)
    expect(onToggleActive).toHaveBeenCalledTimes(1)
    expect(onDelete).toHaveBeenCalledTimes(1)
    expect(onClearProxy).toHaveBeenCalledTimes(1)
    expect(onSetProxy).toHaveBeenCalledWith('proxy-node-2')

    unmount()
  })
})
