import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { createApp, nextTick, type App } from 'vue'

import PoolManagement from '@/views/admin/PoolManagement.vue'
import type { PoolKeyDetail, PoolOverviewItem, PoolKeysPageResponse } from '@/api/endpoints/pool'
import { POOL_MANAGEMENT_VIEW_STORAGE_KEY } from '@/features/pool/utils/poolManagementState'

const endpointMocks = vi.hoisted(() => ({
  getPoolOverview: vi.fn(),
  getPoolSchedulingPresets: vi.fn(),
  listPoolKeys: vi.fn(),
  clearPoolCooldown: vi.fn(),
  getProvider: vi.fn(),
  updateProvider: vi.fn(),
  revealEndpointKey: vi.fn(),
  exportKey: vi.fn(),
  deleteEndpointKey: vi.fn(),
  updateProviderKey: vi.fn(),
  refreshProviderQuota: vi.fn(),
  resetProviderKeyCycleStats: vi.fn(),
  refreshProviderOAuth: vi.fn(),
}))

const routeMocks = vi.hoisted(() => ({
  query: {} as Record<string, string>,
  patchQuery: vi.fn((patch: Record<string, string | undefined | null>) => {
    for (const [key, value] of Object.entries(patch)) {
      if (value == null || String(value).trim() === '') {
        delete routeMocks.query[key]
      } else {
        routeMocks.query[key] = String(value)
      }
    }
  }),
}))

const proxyStoreMocks = vi.hoisted(() => ({
  ensureLoaded: vi.fn(),
}))

vi.mock('@/api/endpoints/pool', () => ({
  getPoolOverview: endpointMocks.getPoolOverview,
  getPoolSchedulingPresets: endpointMocks.getPoolSchedulingPresets,
  listPoolKeys: endpointMocks.listPoolKeys,
  clearPoolCooldown: endpointMocks.clearPoolCooldown,
}))

vi.mock('@/api/endpoints/keys', () => ({
  revealEndpointKey: endpointMocks.revealEndpointKey,
  exportKey: endpointMocks.exportKey,
  deleteEndpointKey: endpointMocks.deleteEndpointKey,
  updateProviderKey: endpointMocks.updateProviderKey,
  refreshProviderQuota: endpointMocks.refreshProviderQuota,
  resetProviderKeyCycleStats: endpointMocks.resetProviderKeyCycleStats,
}))

vi.mock('@/api/endpoints/provider_oauth', () => ({
  refreshProviderOAuth: endpointMocks.refreshProviderOAuth,
}))

vi.mock('@/api/endpoints', () => ({
  getProvider: endpointMocks.getProvider,
  updateProvider: endpointMocks.updateProvider,
}))

vi.mock('@/composables/useRouteQuery', () => ({
  useRouteQuery: () => ({
    getQueryValue: (key: string) => routeMocks.query[key],
    patchQuery: routeMocks.patchQuery,
  }),
}))

vi.mock('@/stores/proxy-nodes', () => ({
  useProxyNodesStore: () => ({
    nodes: [],
    ensureLoaded: proxyStoreMocks.ensureLoaded,
  }),
}))

vi.mock('@/composables/useToast', () => ({
  useToast: () => ({
    success: vi.fn(),
    error: vi.fn(),
    warning: vi.fn(),
  }),
}))

vi.mock('@/composables/useConfirm', () => ({
  useConfirm: () => ({
    confirm: vi.fn().mockResolvedValue(true),
  }),
}))

vi.mock('@/composables/useClipboard', () => ({
  useClipboard: () => ({
    copyToClipboard: vi.fn().mockResolvedValue(undefined),
  }),
}))

vi.mock('@/composables/useCountdownTimer', async () => {
  const { ref } = await import('vue')
  return {
    useCountdownTimer: () => ({
      tick: ref(0),
      start: vi.fn(),
    }),
    getCodexResetCountdown: () => ({
      isExpired: false,
      text: '1h',
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
    Search: Icon,
    Upload: Icon,
    ChevronDown: Icon,
    RefreshCw: Icon,
    Activity: Icon,
    Power: Icon,
    Database: Icon,
    KeyRound: Icon,
    Download: Icon,
    Copy: Icon,
    Shield: Icon,
    Globe: Icon,
    RotateCcw: Icon,
    SquarePen: Icon,
    Trash2: Icon,
    Users: Icon,
    Settings2: Icon,
    SlidersHorizontal: Icon,
    CircleHelp: Icon,
    Edit: Icon,
    Eye: Icon,
    ListChecks: Icon,
    SquareCheckBig: Icon,
  }
})

vi.mock('@/components/ui', async () => {
  const { computed, defineComponent, h, inject, provide } = await import('vue')
  const passthrough = (name: string, tag = 'div') => defineComponent({
    name,
    inheritAttrs: false,
    setup(_, { attrs, slots }) {
      return () => h(tag, attrs, slots.default?.())
    },
  })

  const Button = defineComponent({
    name: 'ButtonStub',
    inheritAttrs: false,
    props: {
      disabled: Boolean,
    },
    setup(props, { attrs, slots }) {
      return () => h('button', { ...attrs, disabled: props.disabled, type: attrs.type ?? 'button' }, slots.default?.())
    },
  })

  const Input = defineComponent({
    name: 'InputStub',
    inheritAttrs: false,
    props: {
      modelValue: { type: [String, Number], default: '' },
    },
    emits: ['update:modelValue'],
    setup(props, { attrs, emit }) {
      return () => h('input', {
        ...attrs,
        value: props.modelValue ?? '',
        onInput: (event: Event) => emit('update:modelValue', (event.target as HTMLInputElement).value),
      })
    },
  })

  const Switch = defineComponent({
    name: 'SwitchStub',
    inheritAttrs: false,
    props: {
      modelValue: Boolean,
    },
    emits: ['update:modelValue'],
    setup(props, { attrs, emit }) {
      return () => h('input', {
        ...attrs,
        type: 'checkbox',
        role: 'switch',
        checked: props.modelValue,
        onChange: (event: Event) => emit('update:modelValue', (event.target as HTMLInputElement).checked),
      })
    },
  })

  const Checkbox = defineComponent({
    name: 'CheckboxStub',
    inheritAttrs: false,
    props: {
      checked: Boolean,
      modelValue: Boolean,
      indeterminate: Boolean,
      disabled: Boolean,
    },
    emits: ['update:checked', 'update:modelValue'],
    setup(props, { attrs, emit }) {
      return () => h('input', {
        ...attrs,
        type: 'checkbox',
        checked: props.checked || props.modelValue,
        disabled: props.disabled,
        'data-indeterminate': props.indeterminate ? 'true' : undefined,
        onChange: (event: Event) => {
          const checked = (event.target as HTMLInputElement).checked
          emit('update:checked', checked)
          emit('update:modelValue', checked)
        },
      })
    },
  })

  const Pagination = defineComponent({
    name: 'PaginationStub',
    setup() {
      return () => h('nav')
    },
  })

  const DropdownMenuItem = defineComponent({
    name: 'DropdownMenuItemStub',
    inheritAttrs: false,
    emits: ['select'],
    setup(_, { attrs, emit, slots }) {
      return () => h('button', {
        ...attrs,
        type: 'button',
        onClick: (event: Event) => emit('select', event),
      }, slots.default?.())
    },
  })

  const popoverContextKey = Symbol('PopoverStubContext')

  const Popover = defineComponent({
    name: 'PopoverStub',
    inheritAttrs: false,
    props: {
      open: Boolean,
    },
    emits: ['update:open'],
    setup(props, { slots, emit }) {
      const context = {
        open: computed(() => props.open),
        toggle: () => emit('update:open', !props.open),
      }
      provide(popoverContextKey, context)
      return () => slots.default?.()
    },
  })

  const PopoverTrigger = defineComponent({
    name: 'PopoverTriggerStub',
    inheritAttrs: false,
    setup(_, { attrs, slots }) {
      const context = inject<{ open: { value: boolean }, toggle: () => void } | null>(popoverContextKey, null)
      return () => {
        return h('span', {
          ...attrs,
          onClickCapture: () => {
            context?.toggle()
          },
        }, slots.default?.())
      }
    },
  })

  const PopoverContent = defineComponent({
    name: 'PopoverContentStub',
    inheritAttrs: false,
    setup(_, { attrs, slots }) {
      const context = inject<{ open: { value: boolean } } | null>(popoverContextKey, null)
      return () => {
        if (!context?.open.value) return null
        return h('div', { ...attrs, 'data-state': 'open' }, slots.default?.())
      }
    },
  })

  return {
    Card: passthrough('CardStub'),
    Badge: passthrough('BadgeStub', 'span'),
    Button,
    Checkbox,
    Input,
    Select: passthrough('SelectStub'),
    SelectTrigger: passthrough('SelectTriggerStub', 'button'),
    SelectValue: passthrough('SelectValueStub', 'span'),
    SelectContent: passthrough('SelectContentStub'),
    SelectItem: passthrough('SelectItemStub'),
    Table: passthrough('TableStub', 'table'),
    TableHeader: passthrough('TableHeaderStub', 'thead'),
    TableBody: passthrough('TableBodyStub', 'tbody'),
    TableRow: passthrough('TableRowStub', 'tr'),
    TableHead: passthrough('TableHeadStub', 'th'),
    SortableTableHead: passthrough('SortableTableHeadStub', 'th'),
    TableFilterMenu: passthrough('TableFilterMenuStub'),
    TableCell: passthrough('TableCellStub', 'td'),
    DropdownMenu: passthrough('DropdownMenuStub'),
    DropdownMenuTrigger: passthrough('DropdownMenuTriggerStub'),
    DropdownMenuContent: passthrough('DropdownMenuContentStub'),
    DropdownMenuItem,
    Switch,
    Pagination,
    Popover,
    PopoverTrigger,
    PopoverContent,
  }
})

vi.mock('@/components/ui/refresh-button.vue', async () => {
  const { defineComponent, h } = await import('vue')
  return {
    default: defineComponent({
      name: 'RefreshButtonStub',
      setup(_, { attrs }) {
        return () => h('button', attrs, '刷新')
      },
    }),
  }
})

vi.mock('@/features/pool/components/PoolSchedulingDialog.vue', async () => {
  const { defineComponent } = await import('vue')
  return {
    default: defineComponent({
      name: 'PoolSchedulingDialogStub',
      setup() {
        return () => null
      },
    }),
  }
})
vi.mock('@/features/pool/components/PoolAdvancedDialog.vue', async () => {
  const { defineComponent } = await import('vue')
  return {
    default: defineComponent({
      name: 'PoolAdvancedDialogStub',
      setup() {
        return () => null
      },
    }),
  }
})
vi.mock('@/features/pool/components/PoolDemandMetricsDialog.vue', async () => {
  const { defineComponent } = await import('vue')
  return {
    default: defineComponent({
      name: 'PoolDemandMetricsDialogStub',
      setup() {
        return () => null
      },
    }),
  }
})
vi.mock('@/features/pool/components/PoolAccountBatchDialog.vue', async () => {
  const { defineComponent, h } = await import('vue')
  return {
    default: defineComponent({
      name: 'PoolAccountBatchDialogStub',
      props: {
        modelValue: Boolean,
        selectedKeys: { type: Array, default: () => [] },
        selectAllFiltered: Boolean,
        selectedCount: { type: Number, default: 0 },
        selectionFilters: { type: Object, default: () => ({}) },
        initialAction: { type: String, default: null },
      },
      emits: ['update:modelValue', 'changed', 'editConfig'],
      setup(props) {
        return () => h('div', {
          'data-testid': 'pool-account-batch-dialog',
          'data-open': props.modelValue ? 'true' : 'false',
          'data-selected-ids': (props.selectedKeys as PoolKeyDetail[])
            .map(key => key.key_id)
            .join(','),
          'data-select-all-filtered': props.selectAllFiltered ? 'true' : 'false',
          'data-selected-count': String(props.selectedCount),
          'data-selection-filters': JSON.stringify(props.selectionFilters),
          'data-initial-action': props.initialAction || '',
        })
      },
    }),
  }
})
vi.mock('@/features/pool/components/PoolKeyBatchEditDialog.vue', async () => {
  const { defineComponent } = await import('vue')
  return {
    default: defineComponent({
      name: 'PoolKeyBatchEditDialogStub',
      setup() {
        return () => null
      },
    }),
  }
})
vi.mock('@/features/providers/components/ProviderFormDialog.vue', async () => {
  const { defineComponent } = await import('vue')
  return {
    default: defineComponent({
      name: 'ProviderFormDialogStub',
      setup() {
        return () => null
      },
    }),
  }
})
vi.mock('@/features/providers/components/KeyAllowedModelsEditDialog.vue', async () => {
  const { defineComponent } = await import('vue')
  return {
    default: defineComponent({
      name: 'KeyAllowedModelsEditDialogStub',
      setup() {
        return () => null
      },
    }),
  }
})
vi.mock('@/features/providers/components/KeyFormDialog.vue', async () => {
  const { defineComponent } = await import('vue')
  return {
    default: defineComponent({
      name: 'KeyFormDialogStub',
      setup() {
        return () => null
      },
    }),
  }
})
vi.mock('@/features/providers/components/OAuthKeyEditDialog.vue', async () => {
  const { defineComponent } = await import('vue')
  return {
    default: defineComponent({
      name: 'OAuthKeyEditDialogStub',
      setup() {
        return () => null
      },
    }),
  }
})
vi.mock('@/features/providers/components/OAuthAccountDialog.vue', async () => {
  const { defineComponent } = await import('vue')
  return {
    default: defineComponent({
      name: 'OAuthAccountDialogStub',
      setup() {
        return () => null
      },
    }),
  }
})
vi.mock('@/features/providers/components/ProxyNodeSelect.vue', async () => {
  const { defineComponent } = await import('vue')
  return {
    default: defineComponent({
      name: 'ProxyNodeSelectStub',
      setup() {
        return () => null
      },
    }),
  }
})

const mountedApps: Array<{ app: App, root: HTMLElement }> = []

function createOverview(providerType: string): PoolOverviewItem {
  return {
    provider_id: `${providerType}-provider`,
    provider_name: `${providerType} Provider`,
    provider_type: providerType,
    total_keys: 1,
    active_keys: 1,
    cooldown_count: 0,
    pool_enabled: true,
  }
}

function createProvider(providerType: string, overrides: Record<string, unknown> = {}) {
  return {
    id: `${providerType}-provider`,
    name: `${providerType} Provider`,
    provider_type: providerType,
    is_active: true,
    api_formats: ['openai:chat'],
    proxy: null,
    pool_advanced: null,
    claude_code_advanced: null,
    ...overrides,
  }
}

function createPoolKey(providerType = 'codex', overrides: Partial<PoolKeyDetail> = {}): PoolKeyDetail {
  return {
    key_id: `${providerType}-key-1`,
    key_name: `${providerType} key`,
    is_active: true,
    auth_type: 'api_key',
    api_formats: ['openai:chat'],
    internal_priority: 50,
    account_quota: null,
    cooldown_reason: null,
    cooldown_ttl_seconds: null,
    cost_window_usage: 0,
    cost_limit: null,
    request_count: 9876,
    total_tokens: 4321000,
    total_cost_usd: '8.7654',
    sticky_sessions: 0,
    lru_score: null,
    created_at: '2026-05-05T00:00:00Z',
    imported_at: '2026-05-05T00:00:00Z',
    last_used_at: '2026-05-05T01:00:00Z',
    status_snapshot: {
      oauth: { code: 'none' },
      account: { code: 'ok', blocked: false },
      quota: {
        code: 'ok',
        exhausted: false,
        provider_type: providerType,
        windows: providerType === 'codex'
          ? [
              {
                code: '5h',
                remaining_ratio: 0.8,
                usage: { request_count: 7, total_tokens: 2500, total_cost_usd: '0.0045' },
              },
              {
                code: 'weekly',
                remaining_ratio: 0.5,
                usage: { request_count: 12, total_tokens: 5000, total_cost_usd: '0.012' },
              },
            ]
          : [],
      },
    },
    ...overrides,
  }
}

function createKeyPage(key: PoolKeyDetail): PoolKeysPageResponse {
  return {
    total: 1,
    page: 1,
    page_size: 50,
    keys: [key],
  }
}

function resetQuery() {
  for (const key of Object.keys(routeMocks.query)) {
    delete routeMocks.query[key]
  }
}

function mountPoolManagement() {
  const root = document.createElement('div')
  document.body.appendChild(root)
  const app = createApp(PoolManagement)
  app.mount(root)
  mountedApps.push({ app, root })
  return root
}

async function settle() {
  for (let index = 0; index < 8; index += 1) {
    await Promise.resolve()
    await nextTick()
  }
}

beforeEach(() => {
  resetQuery()
  window.sessionStorage.clear()
  routeMocks.patchQuery.mockClear()
  proxyStoreMocks.ensureLoaded.mockClear()

  endpointMocks.getPoolOverview.mockReset()
  endpointMocks.getPoolSchedulingPresets.mockReset()
  endpointMocks.listPoolKeys.mockReset()
  endpointMocks.clearPoolCooldown.mockReset()
  endpointMocks.getProvider.mockReset()
  endpointMocks.updateProvider.mockReset()
  endpointMocks.revealEndpointKey.mockReset()
  endpointMocks.exportKey.mockReset()
  endpointMocks.deleteEndpointKey.mockReset()
  endpointMocks.updateProviderKey.mockReset()
  endpointMocks.refreshProviderQuota.mockReset()
  endpointMocks.resetProviderKeyCycleStats.mockReset()
  endpointMocks.refreshProviderOAuth.mockReset()

  endpointMocks.getPoolSchedulingPresets.mockResolvedValue([])
  endpointMocks.clearPoolCooldown.mockResolvedValue({ message: 'ok' })
  endpointMocks.refreshProviderQuota.mockResolvedValue({ success: 0, failed: 0 })
  endpointMocks.resetProviderKeyCycleStats.mockResolvedValue({ message: '已重置周期统计', reset_at: 123, windows: 2 })
})

afterEach(() => {
  for (const { app, root } of mountedApps.splice(0)) {
    app.unmount()
    root.remove()
  }
})

describe('PoolManagement Codex cycle stats mode', () => {
  it('renders current-cycle comparison text without a mode toggle', async () => {
    const codexKey = createPoolKey('codex')
    endpointMocks.getPoolOverview.mockResolvedValue({ items: [createOverview('codex')] })
    endpointMocks.listPoolKeys.mockResolvedValue(createKeyPage(codexKey))
    endpointMocks.getProvider.mockResolvedValue(createProvider('codex'))

    const root = mountPoolManagement()
    await settle()

    expect(root.querySelector('[data-testid="pool-stats-mode-control"]')).toBeNull()
    expect(root.querySelector('[data-testid="pool-stats-cycle-text"]')).not.toBeNull()
    expect(root.querySelector('[data-testid="pool-stats-cycle-request_count"]')?.textContent?.trim()).toBe('7/12')
    expect(root.querySelector('[data-testid="pool-stats-cycle-total_tokens"]')?.textContent?.trim()).toBe('2.5K/5K')
    expect(root.querySelector('[data-testid="pool-stats-cycle-small-overlay"]')).toBeNull()
    expect(root.querySelector('[data-testid="pool-stats-cycle-large-base"]')).toBeNull()
    expect(endpointMocks.listPoolKeys).toHaveBeenLastCalledWith(
      'codex-provider',
      expect.objectContaining({
        sort_by: 'imported_at',
        sort_order: 'desc',
      }),
      expect.anything(),
    )
    expect(root.textContent).not.toContain('累计')
    expect(root.textContent).not.toContain('总计')
  })

  it('renders unified pool score in the key list with a calculation entry point', async () => {
    const scoredKey = createPoolKey('codex', {
      pool_score: {
        id: 'pms-account-score',
        capability: 'account',
        scope_kind: 'account',
        scope_id: null,
        score: 0.875,
        hard_state: 'available',
        score_version: 1,
        score_reason: { weights: { manual_priority: 0.3 } },
        last_ranked_at: 1_700_000_000,
        last_scheduled_at: 1_700_000_010,
        last_success_at: 1_700_000_020,
        last_failure_at: null,
        failure_count: 0,
        last_probe_attempt_at: 1_700_000_030,
        last_probe_success_at: 1_700_000_040,
        last_probe_failure_at: null,
        probe_failure_count: 0,
        probe_status: 'ok',
        updated_at: 1_700_000_050,
      },
    })
    endpointMocks.getPoolOverview.mockResolvedValue({ items: [createOverview('codex')] })
    endpointMocks.listPoolKeys.mockResolvedValue(createKeyPage(scoredKey))
    endpointMocks.getProvider.mockResolvedValue(createProvider('codex'))

    const root = mountPoolManagement()
    await settle()

    expect(root.textContent).toContain('0.875')
    expect(root.querySelectorAll('button[title="查看评分计算结果"]').length).toBeGreaterThan(0)
  })

  it('shows ChatGPT Web image quota reset countdown above the quota bar', async () => {
    const chatgptWebKey = createPoolKey('chatgpt_web', {
      api_formats: ['openai:image'],
      status_snapshot: {
        oauth: { code: 'valid' },
        account: { code: 'ok', blocked: false },
        quota: {
          code: 'ok',
          exhausted: false,
          provider_type: 'chatgpt_web',
          updated_at: 1_700_000_000,
          windows: [
            {
              code: 'image_gen',
              label: '生图',
              scope: 'account',
              remaining_ratio: 0.96,
              remaining_value: 24,
              limit_value: 25,
              reset_seconds: 3600,
            },
          ],
        },
      },
    })
    endpointMocks.getPoolOverview.mockResolvedValue({ items: [createOverview('chatgpt_web')] })
    endpointMocks.listPoolKeys.mockResolvedValue(createKeyPage(chatgptWebKey))
    endpointMocks.getProvider.mockResolvedValue(createProvider('chatgpt_web', {
      api_formats: ['openai:image'],
    }))

    const root = mountPoolManagement()
    await settle()

    const resetTexts = Array.from(root.querySelectorAll('[data-testid="pool-quota-reset-text"]'))
      .map((element) => element.textContent?.trim())
      .filter(Boolean)
    expect(resetTexts).toContain('1h')
    expect(root.textContent).toContain('生图')
  })

  it('labels Codex quota by the actual refresh window duration', async () => {
    const monthlyCodexKey = createPoolKey('codex', {
      status_snapshot: {
        oauth: { code: 'valid' },
        account: { code: 'ok', blocked: false },
        quota: {
          code: 'ok',
          exhausted: false,
          provider_type: 'codex',
          windows: [
            {
              code: 'weekly',
              remaining_ratio: 0.86,
              window_minutes: 43_800,
              usage: { request_count: 23, total_tokens: 45_600, total_cost_usd: '0.1234' },
            },
            {
              code: '5h',
              remaining_ratio: 1,
              window_minutes: 0,
            },
          ],
        },
      },
    })
    endpointMocks.getPoolOverview.mockResolvedValue({ items: [createOverview('codex')] })
    endpointMocks.listPoolKeys.mockResolvedValue(createKeyPage(monthlyCodexKey))
    endpointMocks.getProvider.mockResolvedValue(createProvider('codex'))

    const root = mountPoolManagement()
    await settle()

    const periodLabels = Array.from(root.querySelectorAll('[data-testid="pool-quota-period-label"]'))
      .map((element) => element.textContent?.trim())
      .filter(Boolean)
    expect(periodLabels).toContain('月')
    expect(periodLabels).not.toContain('5H')
    expect(periodLabels).not.toContain('周')
    expect(root.querySelector('[data-testid="pool-stats-cycle-request_count"]')?.textContent?.trim()).toBe('-/23')
    expect(root.querySelector('[data-testid="pool-stats-cycle-small-overlay"]')).toBeNull()
    expect(root.querySelector('[data-testid="pool-stats-cycle-bar-request_count"]')).toBeNull()
    expect(root.querySelector('[data-testid="pool-stats-cycle-single-marker"]')).toBeNull()
  })

  it('shows Codex remaining 0% on the pool table when the account is exhausted', async () => {
    const exhaustedCodexKey = createPoolKey('codex', {
      status_snapshot: {
        oauth: { code: 'valid' },
        account: { code: 'ok', blocked: false },
        quota: {
          code: 'exhausted',
          exhausted: true,
          provider_type: 'codex',
          usage_ratio: 1,
          windows: [
            {
              code: 'weekly',
              remaining_ratio: 1,
              window_minutes: 10_080,
            },
          ],
        },
      },
    })
    endpointMocks.getPoolOverview.mockResolvedValue({ items: [createOverview('codex')] })
    endpointMocks.listPoolKeys.mockResolvedValue(createKeyPage(exhaustedCodexKey))
    endpointMocks.getProvider.mockResolvedValue(createProvider('codex'))

    const root = mountPoolManagement()
    await settle()

    const meter = root.querySelector('[data-testid="pool-quota-meter-text"]')
    expect(meter?.textContent?.trim()).toBe('0.0%')
  })

  it('opens only one score popover across desktop and mobile layouts', async () => {
    const scoredKey = createPoolKey('codex', {
      pool_score: {
        id: 'pms-account-score',
        capability: 'account',
        scope_kind: 'account',
        scope_id: null,
        score: 0.662,
        hard_state: 'available',
        score_version: 1,
        score_reason: {
          rules: {
            probe_failure_penalty: 0.05,
          },
        },
        last_ranked_at: 1_700_000_000,
        last_scheduled_at: null,
        last_success_at: null,
        last_failure_at: null,
        failure_count: 0,
        last_probe_attempt_at: null,
        last_probe_success_at: null,
        last_probe_failure_at: null,
        probe_failure_count: 0,
        probe_status: 'ok',
        updated_at: 1_700_000_050,
      },
    })
    endpointMocks.getPoolOverview.mockResolvedValue({ items: [createOverview('codex')] })
    endpointMocks.listPoolKeys.mockResolvedValue(createKeyPage(scoredKey))
    endpointMocks.getProvider.mockResolvedValue(createProvider('codex'))

    const root = mountPoolManagement()
    await settle()

    const helpButtons = root.querySelectorAll<HTMLButtonElement>('button[title="查看评分计算结果"]')
    expect(helpButtons.length).toBe(2)

    helpButtons[0]?.click()
    await settle()

    expect(root.querySelectorAll('pre').length).toBe(1)
    expect(root.textContent).toContain('评分计算结果')
    expect(root.textContent).toContain('0.662')
  })

  it('refreshes quota only for keys on the current page', async () => {
    const pageKeys = [
      createPoolKey('codex', { key_id: 'codex-page-key-1', quota_updated_at: null }),
      createPoolKey('codex', { key_id: 'codex-page-key-2', quota_updated_at: null }),
    ]
    endpointMocks.getPoolOverview.mockResolvedValue({
      items: [{ ...createOverview('codex'), total_keys: 120 }],
    })
    endpointMocks.listPoolKeys.mockResolvedValue({
      total: 120,
      page: 1,
      page_size: 50,
      keys: pageKeys,
    })
    endpointMocks.getProvider.mockResolvedValue(createProvider('codex'))
    endpointMocks.refreshProviderQuota.mockResolvedValue({
      success: 2,
      failed: 0,
      total: 2,
      results: [],
    })

    const root = mountPoolManagement()
    await settle()

    const refreshButton = root.querySelector('button[title="刷新数据和额度"]') as HTMLButtonElement | null
    expect(refreshButton).not.toBeNull()
    refreshButton?.click()
    await settle()

    expect(endpointMocks.refreshProviderQuota).toHaveBeenCalledTimes(1)
    expect(endpointMocks.refreshProviderQuota).toHaveBeenCalledWith(
      'codex-provider',
      ['codex-page-key-1', 'codex-page-key-2'],
    )
    expect(endpointMocks.refreshProviderQuota).not.toHaveBeenCalledWith('codex-provider')
  })

  it('ignores legacy account-total mode and removes it from the route', async () => {
    window.sessionStorage.setItem(
      POOL_MANAGEMENT_VIEW_STORAGE_KEY,
      JSON.stringify({ statsMode: 'account_total' }),
    )
    routeMocks.query.statsMode = 'account_total'
    const codexKey = createPoolKey('codex')
    endpointMocks.getPoolOverview.mockResolvedValue({ items: [createOverview('codex')] })
    endpointMocks.listPoolKeys.mockResolvedValue(createKeyPage(codexKey))
    endpointMocks.getProvider.mockResolvedValue(createProvider('codex'))

    const root = mountPoolManagement()
    await settle()

    expect(root.querySelector('[data-testid="pool-stats-mode-control"]')).toBeNull()
    expect(root.querySelector('[data-testid="pool-stats-cycle-text"]')).not.toBeNull()
    expect(root.querySelector('[data-testid="pool-stats-account-total"]')).toBeNull()
    expect(routeMocks.query.statsMode).toBeUndefined()
  })

  it('resets Codex cycle stats from the action column', async () => {
    const codexKey = createPoolKey('codex')
    const resetKey = createPoolKey('codex', {
      status_snapshot: {
        ...codexKey.status_snapshot,
        quota: {
          ...codexKey.status_snapshot?.quota,
          windows: [
            {
              code: '5h',
              scope: 'account',
              window_minutes: 300,
              usage_reset_at: 123,
              usage: { request_count: 0, total_tokens: 0, total_cost_usd: '0.00000000' },
            },
            {
              code: 'spark_preview',
              scope: 'account',
              window_minutes: 300,
              usage: { request_count: 12, total_tokens: 500, total_cost_usd: '0.01' },
            },
            {
              code: 'model_window',
              scope: 'model',
              window_minutes: 300,
              usage: { request_count: 9, total_tokens: 400, total_cost_usd: '0.02' },
            },
            {
              code: 'lifetime',
              scope: 'account',
              window_minutes: 0,
              usage: { request_count: 8, total_tokens: 300, total_cost_usd: '0.03' },
            },
          ],
        },
      },
    })
    endpointMocks.getPoolOverview.mockResolvedValue({ items: [createOverview('codex')] })
    endpointMocks.listPoolKeys
      .mockResolvedValueOnce(createKeyPage(codexKey))
      .mockResolvedValue(createKeyPage(resetKey))
    endpointMocks.getProvider.mockResolvedValue(createProvider('codex'))

    const root = mountPoolManagement()
    await settle()

    const resetButton = root.querySelector<HTMLButtonElement>('[data-testid="pool-reset-cycle-stats"]')
    expect(resetButton).not.toBeNull()

    resetButton?.click()
    await settle()

    expect(endpointMocks.resetProviderKeyCycleStats).toHaveBeenCalledWith(codexKey.key_id)
    expect(endpointMocks.listPoolKeys).toHaveBeenCalledTimes(2)
    expect(root.querySelector('[data-testid="pool-stats-cycle-request_count"]')?.textContent?.trim()).toBe('-/0')
  })

  it('toggles a pool account and silently revalidates the current key page', async () => {
    const inactiveKey = createPoolKey('codex', {
      is_active: false,
      cooldown_reason: 'rate_limited_429',
      cooldown_ttl_seconds: 60,
      scheduling_status: 'blocked',
      scheduling_reason: 'inactive',
      scheduling_label: '已禁用',
      scheduling_reasons: [{
        code: 'inactive',
        label: '已禁用',
        blocking: true,
        source: 'manual',
      }],
    })
    const enabledKey = createPoolKey('codex', {
      is_active: true,
      cooldown_reason: 'rate_limited_429',
      cooldown_ttl_seconds: 60,
      scheduling_status: 'degraded',
      scheduling_reason: 'cooldown',
      scheduling_label: '冷却中',
      scheduling_reasons: [{
        code: 'cooldown',
        label: '冷却中',
        blocking: true,
        source: 'pool',
        ttl_seconds: 60,
        detail: 'rate_limited_429',
      }],
    })
    endpointMocks.getPoolOverview.mockResolvedValue({
      items: [{ ...createOverview('codex'), active_keys: 0 }],
    })
    endpointMocks.listPoolKeys
      .mockResolvedValueOnce(createKeyPage(inactiveKey))
      .mockResolvedValue(createKeyPage(enabledKey))
    endpointMocks.getProvider.mockResolvedValue(createProvider('codex', {
      total_keys: 1,
      active_keys: 0,
    }))
    endpointMocks.updateProviderKey.mockResolvedValue({ ...inactiveKey, is_active: true })

    const root = mountPoolManagement()
    await settle()

    const toggleButton = root.querySelector<HTMLButtonElement>(
      `[data-testid="pool-toggle-active-desktop-${inactiveKey.key_id}"]`,
    )
    expect(toggleButton).not.toBeNull()
    expect(toggleButton?.getAttribute('aria-label')).toBe('启用账号')
    const listCallsBeforeToggle = endpointMocks.listPoolKeys.mock.calls.length

    toggleButton?.click()
    await settle()

    expect(endpointMocks.updateProviderKey).toHaveBeenCalledWith(inactiveKey.key_id, { is_active: true })
    expect(toggleButton?.getAttribute('aria-label')).toBe('禁用账号')
    expect(endpointMocks.listPoolKeys).toHaveBeenCalledTimes(listCallsBeforeToggle + 1)
    expect(root.textContent).toContain('冷却中')
  })

  it('revalidates the pool page when OAuth refresh fails after server-side invalidation', async () => {
    const oauthKey = createPoolKey('codex', {
      auth_type: 'oauth',
      oauth_managed: true,
      can_refresh_oauth: true,
      status_snapshot: {
        ...createPoolKey('codex').status_snapshot,
        oauth: { code: 'expired', expires_at: 1 },
      },
    })
    endpointMocks.getPoolOverview.mockResolvedValue({ items: [createOverview('codex')] })
    endpointMocks.listPoolKeys
      .mockResolvedValueOnce(createKeyPage(oauthKey))
      .mockResolvedValue({ total: 0, page: 1, page_size: 50, keys: [] })
    endpointMocks.getProvider.mockResolvedValue(createProvider('codex'))
    endpointMocks.refreshProviderOAuth.mockRejectedValue(new Error('OAuth invalid'))

    const root = mountPoolManagement()
    await settle()

    const refreshButton = Array.from(root.querySelectorAll<HTMLButtonElement>('button'))
      .find(button => button.title === '重新授权')
    expect(refreshButton).not.toBeUndefined()
    refreshButton?.click()
    await settle()

    expect(endpointMocks.refreshProviderOAuth).toHaveBeenCalledWith(oauthKey.key_id)
    expect(endpointMocks.listPoolKeys).toHaveBeenCalledTimes(2)
    expect(root.textContent).not.toContain(oauthKey.key_name)
  })

  it('hides the stats mode switch for non-Codex providers and keeps account totals', async () => {
    const openaiKey = createPoolKey('openai', {
      request_count: 12,
      total_tokens: 3456,
      total_cost_usd: '1.25',
    })
    endpointMocks.getPoolOverview.mockResolvedValue({ items: [createOverview('openai')] })
    endpointMocks.listPoolKeys.mockResolvedValue(createKeyPage(openaiKey))
    endpointMocks.getProvider.mockResolvedValue(createProvider('openai'))

    const root = mountPoolManagement()
    await settle()

    expect(root.querySelector('[data-testid="pool-stats-mode-control"]')).toBeNull()
    expect(root.querySelector('[data-testid="pool-reset-cycle-stats"]')).toBeNull()
    expect(root.querySelector('[data-testid="pool-stats-cycle-text"]')).toBeNull()
    expect(root.querySelector('[data-testid="pool-stats-account-total"]')).not.toBeNull()
    expect(root.textContent).toContain('12')
    expect(root.textContent).toContain('3.5K')
    expect(root.textContent).toContain('$1.25')
  })

  it('supports page selection across desktop/mobile rows and seeds batch actions', async () => {
    const firstKey = createPoolKey('codex', { key_id: 'codex-selection-1', key_name: 'First key' })
    const secondKey = createPoolKey('codex', { key_id: 'codex-selection-2', key_name: 'Second key' })
    endpointMocks.getPoolOverview.mockResolvedValue({ items: [createOverview('codex')] })
    endpointMocks.listPoolKeys.mockResolvedValue({
      total: 2,
      page: 1,
      page_size: 50,
      keys: [firstKey, secondKey],
    })
    endpointMocks.getProvider.mockResolvedValue(createProvider('codex'))

    const root = mountPoolManagement()
    await settle()

    let pageCheckbox = root.querySelector<HTMLInputElement>('[data-testid="pool-select-page-desktop"]')
    const firstDesktopCheckbox = root.querySelector<HTMLInputElement>('[data-testid="pool-select-desktop-codex-selection-1"]')
    const firstMobileCheckbox = root.querySelector<HTMLInputElement>('[data-testid="pool-select-mobile-codex-selection-1"]')
    expect(pageCheckbox).not.toBeNull()
    expect(firstDesktopCheckbox).not.toBeNull()
    expect(firstMobileCheckbox).not.toBeNull()
    expect(root.querySelector('[data-testid="pool-selected-count-desktop"]')).toBeNull()
    expect(root.querySelector('[data-testid="pool-selected-count-mobile"]')).toBeNull()
    expect(root.querySelector<HTMLElement>('colgroup col')?.style.width).toBe('19%')
    expect(root.querySelectorAll('colgroup col')).toHaveLength(8)
    expect(pageCheckbox?.closest('th')?.textContent).toContain('名称')
    expect(firstDesktopCheckbox?.closest('td')?.textContent).toContain('First key')

    firstDesktopCheckbox?.click()
    await settle()

    expect(firstDesktopCheckbox?.checked).toBe(true)
    expect(firstMobileCheckbox?.checked).toBe(true)
    expect(pageCheckbox?.dataset.indeterminate).toBe('true')
    const desktopSelectedCount = root.querySelector('[data-testid="pool-selected-count-desktop"]')
    expect(desktopSelectedCount?.textContent).toContain('已选 1 个')
    expect(desktopSelectedCount?.closest('th')).toBe(pageCheckbox?.closest('th'))
    expect(root.querySelector('[data-testid="pool-selected-count-mobile"]')?.textContent).toContain('已选 1 个')
    expect(root.querySelector('[data-testid="pool-selection-toolbar"]')).toBeNull()
    expect(root.querySelector<HTMLButtonElement>('[data-testid="pool-batch-actions-desktop"]')?.disabled).toBe(false)

    pageCheckbox?.click()
    await settle()
    expect(root.querySelector<HTMLInputElement>('[data-testid="pool-select-desktop-codex-selection-2"]')?.checked).toBe(true)
    expect(root.querySelector('[data-testid="pool-selected-count-desktop"]')?.textContent).toContain('已选 2 个')

    pageCheckbox?.click()
    await settle()
    expect(root.querySelector<HTMLButtonElement>('[data-testid="pool-batch-actions-desktop"]')?.disabled).toBe(true)
    expect(root.querySelector('[data-testid="pool-selected-count-desktop"]')).toBeNull()
    expect(root.querySelector('[data-testid="pool-selected-count-mobile"]')).toBeNull()

    pageCheckbox = root.querySelector<HTMLInputElement>('[data-testid="pool-select-page-desktop"]')
    pageCheckbox?.click()
    await settle()
    root.querySelector<HTMLButtonElement>('[data-testid="pool-batch-action-refresh_quota-desktop"]')?.click()
    await settle()

    const batchDialog = root.querySelector('[data-testid="pool-account-batch-dialog"]')
    expect(batchDialog?.getAttribute('data-open')).toBe('true')
    expect(batchDialog?.getAttribute('data-selected-ids')).toBe('codex-selection-1,codex-selection-2')
    expect(batchDialog?.getAttribute('data-select-all-filtered')).toBe('false')
    expect(batchDialog?.getAttribute('data-initial-action')).toBe('refresh_quota')
  })

  it('passes the current table search and status to filtered selection actions', async () => {
    routeMocks.query.providerId = 'codex-provider'
    routeMocks.query.search = 'inactive-account'
    routeMocks.query.status = 'inactive'
    const firstKey = createPoolKey('codex', { key_id: 'codex-filtered-1', key_name: 'inactive-account one' })
    const secondKey = createPoolKey('codex', { key_id: 'codex-filtered-2', key_name: 'inactive-account two' })
    endpointMocks.getPoolOverview.mockResolvedValue({ items: [createOverview('codex')] })
    endpointMocks.listPoolKeys.mockResolvedValue({
      total: 37,
      page: 1,
      page_size: 50,
      keys: [firstKey, secondKey],
    })
    endpointMocks.getProvider.mockResolvedValue(createProvider('codex'))

    const root = mountPoolManagement()
    await settle()

    const selectAllButton = root.querySelector<HTMLButtonElement>('[data-testid="pool-select-all-desktop"]')
    expect(selectAllButton).not.toBeNull()
    expect(selectAllButton?.title).toBe('全选')
    selectAllButton?.click()
    await settle()

    expect(selectAllButton?.title).toBe('取消全选')
    expect(selectAllButton?.getAttribute('aria-pressed')).toBe('true')
    expect(root.querySelector<HTMLInputElement>('[data-testid="pool-select-desktop-codex-filtered-1"]')?.checked).toBe(true)
    expect(root.querySelector<HTMLInputElement>('[data-testid="pool-select-desktop-codex-filtered-1"]')?.disabled).toBe(true)
    expect(root.querySelector('[data-testid="pool-selected-count-desktop"]')?.textContent).toContain('已选 37 个')
    expect(root.querySelector('[data-testid="pool-selected-count-mobile"]')?.textContent).toContain('已选 37 个')
    const batchButton = root.querySelector<HTMLButtonElement>('[data-testid="pool-batch-actions-desktop"]')
    expect(batchButton?.disabled).toBe(false)

    selectAllButton?.click()
    await settle()
    expect(selectAllButton?.title).toBe('全选')
    expect(selectAllButton?.getAttribute('aria-pressed')).toBe('false')
    expect(root.querySelector<HTMLInputElement>('[data-testid="pool-select-desktop-codex-filtered-1"]')?.checked).toBe(false)
    expect(root.querySelector('[data-testid="pool-selected-count-desktop"]')).toBeNull()
    expect(batchButton?.disabled).toBe(true)

    selectAllButton?.click()
    await settle()
    root.querySelector<HTMLButtonElement>('[data-testid="pool-batch-action-refresh_quota-desktop"]')?.click()
    await settle()

    const batchDialog = root.querySelector('[data-testid="pool-account-batch-dialog"]')
    expect(batchDialog?.getAttribute('data-select-all-filtered')).toBe('true')
    expect(batchDialog?.getAttribute('data-selected-count')).toBe('37')
    expect(batchDialog?.getAttribute('data-initial-action')).toBe('refresh_quota')
    expect(JSON.parse(batchDialog?.getAttribute('data-selection-filters') || '{}')).toEqual({
      search: 'inactive-account',
      status: 'inactive',
    })
  })

  it('disables selection while a debounced search is waiting for fresh rows', async () => {
    const key = createPoolKey('codex', { key_id: 'codex-stale-search-row' })
    endpointMocks.getPoolOverview.mockResolvedValue({ items: [createOverview('codex')] })
    endpointMocks.listPoolKeys.mockResolvedValue(createKeyPage(key))
    endpointMocks.getProvider.mockResolvedValue(createProvider('codex'))

    const root = mountPoolManagement()
    await settle()

    const rowCheckbox = root.querySelector<HTMLInputElement>('[data-testid="pool-select-desktop-codex-stale-search-row"]')
    expect(rowCheckbox?.disabled).toBe(false)
    const searchInput = root.querySelector<HTMLInputElement>('input[placeholder="搜索账号..."]')
    expect(searchInput).not.toBeNull()
    if (searchInput) {
      searchInput.value = 'fresh-filter'
      searchInput.dispatchEvent(new Event('input', { bubbles: true }))
    }
    await nextTick()

    expect(rowCheckbox?.disabled).toBe(true)
    expect(root.querySelector<HTMLButtonElement>('[data-testid="pool-select-all-desktop"]')?.disabled).toBe(true)
  })

  it('shows adaptive hot pool metrics entry only when probing is enabled', async () => {
    endpointMocks.getPoolOverview.mockResolvedValue({
      items: [{ ...createOverview('codex'), provider_desired_hot: 4, provider_in_flight: 2, provider_ema_in_flight: 1.8 }],
    })
    endpointMocks.listPoolKeys.mockResolvedValue(createKeyPage(createPoolKey('codex')))
    endpointMocks.getProvider.mockResolvedValue(createProvider('codex', {
      pool_advanced: {
        probing_enabled: true,
      },
    }))

    const enabledRoot = mountPoolManagement()
    await settle()

    expect(enabledRoot.querySelectorAll('[data-testid="pool-demand-metrics-button"]').length).toBeGreaterThan(0)

    for (const { app, root } of mountedApps.splice(0)) {
      app.unmount()
      root.remove()
    }

    endpointMocks.getProvider.mockResolvedValue(createProvider('codex', {
      pool_advanced: {
        probing_enabled: false,
      },
    }))

    const disabledRoot = mountPoolManagement()
    await settle()

    expect(disabledRoot.querySelector('[data-testid="pool-demand-metrics-button"]')).toBeNull()
  })
})
