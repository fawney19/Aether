import { afterEach, describe, expect, it, vi } from 'vitest'
import { createApp, defineComponent, h, nextTick, reactive, type App } from 'vue'
import UsageRecordsTable from '../UsageRecordsTable.vue'
import type { UsageRecord } from '../../types'

vi.mock('@/components/ui', async () => {
  const { defineComponent, h } = await import('vue')

  const passthrough = (name: string, tag = 'div') => defineComponent({
    name,
    setup(_, { slots }) {
      return () => h(tag, [
        slots.default?.(),
        slots.actions?.(),
        slots.pagination?.(),
        slots.filter?.({ close: () => undefined }),
      ])
    },
  })

  return {
    TableCard: passthrough('TableCardStub', 'section'),
    Badge: passthrough('BadgeStub', 'span'),
    Button: passthrough('ButtonStub', 'button'),
    Input: defineComponent({
      name: 'InputStub',
      props: { modelValue: String },
      emits: ['update:modelValue'],
      setup(props, { attrs, emit }) {
        return () => h('input', {
          ...attrs,
          value: props.modelValue ?? '',
          onInput: (event: Event) => emit('update:modelValue', (event.target as HTMLInputElement).value),
        })
      },
    }),
    Select: passthrough('SelectStub'),
    SelectTrigger: passthrough('SelectTriggerStub'),
    SelectValue: passthrough('SelectValueStub', 'span'),
    SelectContent: passthrough('SelectContentStub'),
    SelectItem: passthrough('SelectItemStub'),
    Table: passthrough('TableStub', 'table'),
    TableHeader: passthrough('TableHeaderStub', 'thead'),
    TableBody: passthrough('TableBodyStub', 'tbody'),
    TableRow: passthrough('TableRowStub', 'tr'),
    TableHead: passthrough('TableHeadStub', 'th'),
    TableCell: passthrough('TableCellStub', 'td'),
    Pagination: passthrough('PaginationStub'),
    SortableTableHead: passthrough('SortableTableHeadStub', 'th'),
    TableFilterMenu: defineComponent({
      name: 'TableFilterMenuStub',
      props: { options: Array },
      setup(props) {
        return () => h('div', { 'data-table-filter-menu': '' },
          (props.options as Array<{ value: string, label: string }> | undefined)
            ?.map(option => h('div', { 'data-filter-value': option.value }, option.label)))
      },
    }),
  }
})

vi.mock('@/components/common', async () => {
  const { defineComponent, h } = await import('vue')

  return {
    MultiSelect: defineComponent({
      name: 'MultiSelectStub',
      setup() {
        return () => h('div')
      },
    }),
    TimeRangePicker: defineComponent({
      name: 'TimeRangePickerStub',
      setup() {
        return () => h('div')
      },
    }),
  }
})

vi.mock('lucide-vue-next', async () => {
  const { defineComponent, h } = await import('vue')
  const Icon = defineComponent({
    name: 'IconStub',
    setup(_, { attrs }) {
      return () => h('span', attrs)
    },
  })

  return {
    RefreshCcw: Icon,
    EyeOff: Icon,
    Search: Icon,
    Shuffle: Icon,
    Ban: Icon,
    ShieldCheck: Icon,
    ChevronDown: Icon,
    Check: Icon,
  }
})

vi.mock('../ElapsedTimeText.vue', () => ({
  default: defineComponent({
    name: 'ElapsedTimeTextStub',
    setup() {
      return () => h('span', 'elapsed')
    },
  }),
}))

vi.mock('../ServerUserSelector.vue', () => ({
  default: defineComponent({
    name: 'ServerUserSelectorStub',
    setup() {
      return () => h('div', 'user selector')
    },
  }),
}))

const mountedApps: Array<{ app: App, root: HTMLElement }> = []

function buildRecord(overrides: Partial<UsageRecord> = {}): UsageRecord {
  return {
    id: 'usage-1',
    model: 'gpt-5',
    input_tokens: 100,
    output_tokens: 50,
    total_tokens: 150,
    cost: 0.01,
    response_time_ms: 1000,
    first_byte_time_ms: 500,
    is_stream: true,
    upstream_is_stream: true,
    status: 'completed',
    created_at: '2026-05-06T12:00:00Z',
    ...overrides,
  }
}

function mountUsageRecordsTable(records: UsageRecord[], overrides: Record<string, unknown> = {}) {
  const root = document.createElement('div')
  document.body.appendChild(root)

  const app = createApp(UsageRecordsTable, {
    records,
    isAdmin: true,
    showActualCost: false,
    loading: false,
    timeRange: { preset: 'today', tz_offset_minutes: 0 },
    filterSearch: '',
    filterUser: '__all__',
    filterModel: '__all__',
    filterProvider: '__all__',
    filterApiFormat: '__all__',
    filterStatus: '__all__',
    filterClientFamily: '__all__',
    availableUsers: [],
    availableModels: [],
    availableProviders: [],
    availableClientFamilies: [],
    currentPage: 1,
    pageSize: 20,
    totalRecords: records.length,
    pageSizeOptions: [20, 50],
    autoRefresh: false,
    hideUnknownRecords: false,
    ...overrides,
  })

  app.mount(root)
  mountedApps.push({ app, root })
  return root
}

function expectServiceTierBadge(root: HTMLElement, label: string): HTMLElement {
  const badge = [...root.querySelectorAll<HTMLElement>('span')]
    .find(element => element.textContent?.trim() === label)

  expect(badge).toBeDefined()
  return badge as HTMLElement
}

afterEach(() => {
  for (const { app, root } of mountedApps.splice(0)) {
    app.unmount()
    root.remove()
  }
  vi.useRealTimers()
})

describe('UsageRecordsTable', () => {
  it('replaces ordinary waiting with queued in desktop and mobile rows until admission', async () => {
    const record = reactive(buildRecord({ status: 'pending', first_byte_time_ms: null, is_queued: true }))
    const root = mountUsageRecordsTable([record])
    expect([...root.querySelectorAll('[data-usage-queue-status]')].map(node => node.textContent?.trim()))
      .toEqual(['排队中', '排队中'])

    record.is_queued = false
    await nextTick()

    expect(root.querySelector('[data-usage-queue-status]')).toBeNull()
    expect(root.textContent).toContain('等待中')
  })

  it.each([
    [{ routing_group_name: '生产策略', routing_group_id: 'group-1' }, '生产策略'],
    [{ routing_group_name: ' ', routing_group_id: 'group-history' }, 'group-history'],
    [{}, '未记录分组'],
  ])('shows group then the provider Key in desktop and mobile layouts', (group, expected) => {
    const root = mountUsageRecordsTable([buildRecord({
      ...group,
      provider: '上游 A',
      provider_key_name: '供应商 Key',
      api_key: { id: 'user-key', name: '用户 API Key', display: 'sk-user' },
    })])
    expect([...root.querySelectorAll('[data-usage-provider="routing-group"]')].map(element => element.textContent?.trim()))
      .toEqual([expected, expected])
    const providerLines = [...root.querySelectorAll<HTMLElement>('[data-usage-provider="provider-key"]')]
    expect(providerLines.map(element => element.textContent?.trim())).toEqual(['上游 A · 供应商 Key', '上游 A · 供应商 Key'])
    for (const providerLine of providerLines) {
      expect(providerLine.previousElementSibling?.getAttribute('data-usage-provider')).toBe('routing-group')
      expect(providerLine.title).toBe('上游 A · 供应商 Key')
    }
  })

  it('does not substitute a user API Key when the provider Key is unavailable', () => {
    const root = mountUsageRecordsTable([buildRecord({
      provider: '上游 A',
      api_key: { id: 'user-key', name: '用户 API Key', display: 'sk-user' },
    })])
    expect([...root.querySelectorAll('[data-usage-provider="provider-key"]')].map(element => element.textContent?.trim()))
      .toEqual(['上游 A · -', '上游 A · -'])
  })

  it.each([
    [0, '$0.00'],
    [1, '$10.00'],
    [0.5, '$5.00'],
    [2, '$20.00'],
    [undefined, '$3.00'],
  ])('shows customer charges in both layouts for multiplier %s, including legacy charges', (multiplier, expected) => {
    const root = mountUsageRecordsTable([buildRecord({
      cost: 10,
      actual_cost: 3,
      rate_multiplier: 0.3,
      billing_multiplier: multiplier as number | undefined,
    })], { isAdmin: false })
    const subtitles = [...root.querySelectorAll('[data-usage-cost="routing-group"]')]
    expect(subtitles).toHaveLength(2)
    expect(subtitles.map(element => element.textContent?.trim())).toEqual([expected, expected])
    expect(subtitles.every(element => element.getAttribute('title') === '实际扣费')).toBe(true)
    expect([...root.querySelectorAll('[data-usage-cost="base"]')].map(element => element.textContent?.trim())).toEqual(['$10.00', '$10.00'])
    expect(root.querySelector('[data-usage-cost="provider-key"]')).toBeNull()
    expect(root.querySelector('[data-usage-provider]')).toBeNull()
  })

  it('uses the historical customer charge independently of the administrator-only provider Key cost', () => {
    const root = mountUsageRecordsTable([buildRecord({
      cost: 10,
      actual_cost: 3,
      rate_multiplier: 0.3,
      billing_multiplier: 2,
      billing_cost: 19.99,
    })], { showActualCost: true })
    const subtitles = [...root.querySelectorAll('[data-usage-cost="routing-group"]')]
    expect(subtitles.map(element => element.textContent?.trim())).toEqual(['$19.99', '$19.99'])
    const keyCosts = [...root.querySelectorAll<HTMLElement>('[data-usage-cost="provider-key"]')]
    expect(keyCosts.map(element => element.textContent?.trim())).toEqual(['$3.00', '$3.00'])
    expect(keyCosts.every(element => element.title.includes('提供商 Key 成本'))).toBe(true)
  })

  it('does not replace an unavailable customer charge with the base or provider Key cost', () => {
    const root = mountUsageRecordsTable([buildRecord({
      cost: 10,
      actual_cost: 3,
      rate_multiplier: 0.3,
      billing_multiplier: 2,
      billing_cost: null,
    })], { showActualCost: true })
    expect(root.querySelector('[data-usage-cost="routing-group"]')).toBeNull()
    expect([...root.querySelectorAll('[data-usage-cost="base"]')].map(element => element.textContent?.trim()))
      .toEqual(['$10.00', '$10.00'])
    expect([...root.querySelectorAll('[data-usage-cost="provider-key"]')].map(element => element.textContent?.trim()))
      .toEqual(['$3.00', '$3.00'])
  })

  it.each(['usage_available', 'usage_pricing_available'] as const)('hides all numeric costs when %s is false', field => {
    const root = mountUsageRecordsTable([buildRecord({
      [field]: false,
      actual_cost: 3,
      rate_multiplier: 0.3,
      billing_multiplier: 2,
      billing_cost: 0.02,
    })], { showActualCost: true })
    expect(root.querySelector('[data-usage-cost]')).toBeNull()
    expect(root.querySelectorAll(field === 'usage_available' ? '[data-usage-unavailable="cost"]' : '[data-usage-unpriced="cost"]')).toHaveLength(2)
  })

  it('shows output TPS after the request completes', () => {
    const root = mountUsageRecordsTable([buildRecord()])

    expect(root.textContent).toContain('输出速度')
    expect(root.textContent).toContain('0.50s / 1.00s')
    expect(root.textContent).not.toContain('500ms')
    expect(root.textContent).toContain('100 tps')
    expect([...root.querySelectorAll<HTMLElement>('.text-muted-foreground')]
      .some((element) => element.textContent?.includes('100 tps'))).toBe(true)
    const tpsElements = [...root.querySelectorAll<HTMLElement>('.text-muted-foreground')]
      .filter((element) => element.textContent?.trim() === '100 tps')
    expect(tpsElements.some((element) => element.classList.contains('text-[11px]'))).toBe(false)

    const titles = [...root.querySelectorAll<HTMLElement>('[title]')]
      .map((element) => element.getAttribute('title'))
    expect(titles).toContain([
      '端到端首字: 0.50s',
      '端到端总耗时: 1.00s',
      '成功候选首字: 0.50s',
      '成功候选耗时: 1.00s',
      '生成耗时: 0.50s',
      '输出速度: 100 tokens/s',
    ].join('\n'))
    expect(titles.join('\n')).not.toContain('500ms')
    expect(titles.join('\n')).not.toContain('首字后生成耗时')
  })

  it('shows end-to-end latency while keeping output TPS scoped to the successful candidate', () => {
    const root = mountUsageRecordsTable([buildRecord({
      output_tokens: 50,
      response_time_ms: 626,
      first_byte_time_ms: 100,
      end_to_end_time_ms: 10_626,
      end_to_end_first_byte_time_ms: 10_120,
    })])

    const performanceCell = root.querySelector('table tbody tr td:last-child') as HTMLElement
    expect(performanceCell.textContent).toContain('10.12s / 10.63s')
    expect(performanceCell.textContent).toContain('95.1 tps')
    expect(performanceCell.textContent).not.toContain('98.8 tps')

    const titles = [...root.querySelectorAll<HTMLElement>('[title]')]
      .map((element) => element.getAttribute('title'))
    expect(titles).toContain([
      '端到端首字: 10.12s',
      '端到端总耗时: 10.63s',
      '成功候选首字: 0.10s',
      '成功候选耗时: 0.63s',
      '生成耗时: 0.53s',
      '输出速度: 95.1 tokens/s',
    ].join('\n'))
  })

  it('shows end-to-end latency when candidate timing fields are unavailable', () => {
    const root = mountUsageRecordsTable([buildRecord({
      response_time_ms: null,
      first_byte_time_ms: null,
      end_to_end_time_ms: 10_626,
      end_to_end_first_byte_time_ms: 10_120,
    })])

    const performanceCell = root.querySelector('table tbody tr td:last-child') as HTMLElement
    expect(performanceCell.textContent).toContain('10.12s / 10.63s')

    const titles = [...root.querySelectorAll<HTMLElement>('[title]')]
      .map((element) => element.getAttribute('title'))
    expect(titles).toContain([
      '端到端首字: 10.12s',
      '端到端总耗时: 10.63s',
      '成功候选首字: -',
      '成功候选耗时: -',
      '生成耗时: -',
      '输出速度: -',
    ].join('\n'))
  })

  it('shows an output speed placeholder when the rate is unavailable', () => {
    const root = mountUsageRecordsTable([buildRecord({
      output_tokens: 0,
      response_time_ms: 1000,
      first_byte_time_ms: 500,
    })])

    const performanceCell = root.querySelector('table tbody tr td:last-child') as HTMLElement
    expect(performanceCell.textContent).toContain('0.50s / 1.00s')
    expect([...performanceCell.querySelectorAll<HTMLElement>('.text-muted-foreground')]
      .some((element) => element.textContent?.trim() === '-')).toBe(true)

    const titles = [...root.querySelectorAll<HTMLElement>('[title]')].map((element) => element.title)
    expect(titles).toContain([
      '端到端首字: 0.50s',
      '端到端总耗时: 1.00s',
      '成功候选首字: 0.50s',
      '成功候选耗时: 1.00s',
      '生成耗时: 0.50s',
      '输出速度: -',
    ].join('\n'))
  })

  it('keeps active request latency in one first-byte / live-total line without TPS', () => {
    const root = mountUsageRecordsTable([buildRecord({
      status: 'streaming',
      response_time_ms: null,
      first_byte_time_ms: 500,
    })])

    expect(root.textContent).toContain('0.50s')
    expect(root.textContent).toContain('elapsed')
    expect(root.textContent).toContain('0.50s / elapsed')
    expect(root.textContent).not.toContain('100 tps')
    expect(root.textContent).not.toContain('生成中')
    expect(root.textContent).not.toContain('等待首字')
    expect(root.querySelector('[data-active-latency-state="streaming"]')).toBeNull()
  })

  it('uses a first-byte placeholder and live total before the first byte arrives', () => {
    const root = mountUsageRecordsTable([buildRecord({
      status: 'pending',
      response_time_ms: null,
      first_byte_time_ms: null,
    })])

    expect(root.textContent).toContain('- / elapsed')
    expect(root.textContent).toContain('elapsed')
    expect(root.textContent).not.toContain('等待首字')
    expect(root.querySelector('[data-active-latency-state="waiting-first-byte"]')).toBeNull()
  })

  it('shows failed when Codex image progress fails before the usage record finalizes', () => {
    const root = mountUsageRecordsTable([buildRecord({
      status: 'pending',
      response_time_ms: null,
      first_byte_time_ms: null,
      image_progress: {
        phase: 'failed',
      },
    })])

    expect(root.textContent).toContain('失败')
    expect(root.textContent).not.toContain('等待中')
  })

  it('shows failed instead of waiting when an active row has an HTTP error code', () => {
    const root = mountUsageRecordsTable([buildRecord({
      status: 'pending',
      status_code: 524,
      error_message: 'error code: 524',
      response_time_ms: null,
      first_byte_time_ms: null,
    })])

    expect(root.textContent).toContain('失败')
    expect(root.textContent).not.toContain('等待中')
  })

  it('renders output TPS in the non-admin usage table', () => {
    const root = mountUsageRecordsTable([buildRecord()], { isAdmin: false })

    expect(root.textContent).toContain('100 tps')
    expect(root.textContent).toContain('0.50s / 1.00s')
    expect(root.textContent).toContain('gpt-5')
  })

  it('keeps the type label as WS after Responses detail enriches transport metadata', () => {
    const root = mountUsageRecordsTable([buildRecord({
      is_websocket: true,
      websocket_transport: 'responses',
      status: 'completed',
    })])

    const badges = [...root.querySelectorAll<HTMLElement>('[data-usage-transport="websocket"]')]
    expect(badges.length).toBeGreaterThan(0)
    expect(badges.every(badge => badge.textContent?.trim() === 'WS')).toBe(true)
    expect(badges.every(badge => badge.title === 'OpenAI Responses WebSocket')).toBe(true)
  })

  it('falls back to WS when transport metadata is unavailable', () => {
    const root = mountUsageRecordsTable([buildRecord({
      is_websocket: true,
      websocket_transport: null,
      status: 'completed',
    })])

    const badges = [...root.querySelectorAll<HTMLElement>('[data-usage-transport="websocket"]')]
    expect(badges.length).toBeGreaterThan(0)
    expect(badges.every(badge => badge.textContent?.trim() === 'WS')).toBe(true)
    expect(badges.every(badge => badge.title === 'WebSocket')).toBe(true)
  })

  it('offers a WebSocket type filter in both filter menus', () => {
    const root = mountUsageRecordsTable([buildRecord()])

    const labels = [...root.querySelectorAll<HTMLElement>('div')]
      .filter(element => element.textContent?.trim() === 'WebSocket (WS)')
    expect(labels.length).toBeGreaterThanOrEqual(2)
    expect(root.textContent).toContain('HTTP 流式')
    expect(root.textContent).toContain('HTTP 标准')
  })

  it('keeps OpenAI Live transport rows under the single WS type without inventing usage', () => {
    const root = mountUsageRecordsTable([buildRecord({
      is_websocket: true,
      websocket_transport: 'codex_live_direct',
      usage_available: false,
      input_tokens: 0,
      output_tokens: 0,
      total_tokens: 0,
      cost: 0,
    })])

    const badges = [...root.querySelectorAll<HTMLElement>('[data-usage-transport="websocket"]')]
    expect(badges.length).toBeGreaterThan(0)
    expect(badges.every(badge => badge.textContent?.trim() === 'WS')).toBe(true)
    expect(badges.every(badge => badge.title === 'OpenAI Live 直连 WebSocket')).toBe(true)

    const unavailableTokens = [...root.querySelectorAll<HTMLElement>('[data-usage-unavailable="tokens"]')]
    const unavailableCosts = [...root.querySelectorAll<HTMLElement>('[data-usage-unavailable="cost"]')]
    expect(unavailableTokens.length).toBeGreaterThanOrEqual(2)
    expect(unavailableCosts.length).toBeGreaterThanOrEqual(2)
    expect(unavailableTokens.every(element => element.textContent?.trim() === '不可用')).toBe(true)
    expect(unavailableCosts.every(element => element.textContent?.trim() === '不可用')).toBe(true)
  })

  it('keeps OpenAI Realtime rows under the single WS type in both layouts', () => {
    const root = mountUsageRecordsTable([buildRecord({
      is_websocket: true,
      websocket_transport: 'openai_realtime',
    })])

    const badges = [...root.querySelectorAll<HTMLElement>('[data-usage-transport="websocket"]')]
    expect(badges.length).toBeGreaterThan(0)
    expect(badges.every(badge => badge.textContent?.trim() === 'WS')).toBe(true)
    expect(badges.every(badge => badge.title === 'OpenAI Realtime WebSocket')).toBe(true)
  })

  it('shows reasoning effort next to the model name', () => {
    const root = mountUsageRecordsTable([buildRecord({
      requested_reasoning_effort: 'xhigh',
      reasoning_effort: 'xhigh',
      service_tier: 'priority',
    })])

    expect(root.textContent).toContain('gpt-5')
    expect(root.textContent).toContain('xhigh')
    const inlineLayout = root.querySelector('[data-usage-model-layout="inline"]')
    expect(inlineLayout).not.toBeNull()
    expect(inlineLayout?.querySelector('[data-usage-model-badge="reasoning"]')?.textContent?.trim())
      .toBe('xhigh')
    expect(inlineLayout?.querySelector('[data-usage-model-badge="fast"]')?.textContent?.trim())
      .toBe('Fast')
  })

  it('shows the Gemini thinkingLevel reasoning effort next to the model name', () => {
    const root = mountUsageRecordsTable([buildRecord({
      model: 'gemini-3.8-flash',
      requested_reasoning_effort: 'high',
      reasoning_effort: 'high',
    })])

    expect(root.textContent).toContain('gemini-3.8-flash')
    const badge = root.querySelector('[data-usage-model-badge="reasoning"]')
    expect(badge?.textContent?.trim()).toBe('high')
    expect(badge?.getAttribute('title')).toBe('Reasoning: high')
  })

  it('shows the Gemini thinkingLevel mapping when request and provider disagree', () => {
    const root = mountUsageRecordsTable([buildRecord({
      model: 'gemini-3.8-flash',
      requested_reasoning_effort: 'xhigh',
      reasoning_effort: 'high',
    })])

    expect(root.textContent).toContain('xhigh -> high')
    expect(root.querySelector('[data-usage-model-badge="reasoning"]')?.textContent?.trim())
      .toBe('xhigh -> high')
  })

  it('shows a disabled Gemini thinkingBudget as none', () => {
    const root = mountUsageRecordsTable([buildRecord({
      model: 'gemini-3.8-flash',
      requested_reasoning_effort: null,
      reasoning_effort: 'none',
    })])

    expect(root.querySelector('[data-usage-model-badge="reasoning"]')?.textContent?.trim())
      .toBe('none')
  })

  it('shows request reasoning effort while the record is pending', () => {
    const root = mountUsageRecordsTable([buildRecord({
      status: 'pending',
      requested_reasoning_effort: 'max',
      reasoning_effort: null,
    })])

    expect(root.querySelector('[data-usage-model-badge="reasoning"]')?.textContent?.trim())
      .toBe('max')
  })

  it('marks Responses compaction while the record is pending', () => {
    const root = mountUsageRecordsTable([buildRecord({
      status: 'pending',
      request_type: 'compact',
    })])

    expect(root.querySelector('[data-usage-model-badge="compact"]')?.textContent?.trim())
      .toBe('会话压缩')
  })

  it('shows mapping and response models as separately labelled facts', () => {
    const root = mountUsageRecordsTable([buildRecord({
      model: 'gpt-5',
      target_model: 'gpt-5.1',
      response_model: 'gpt-5.2',
      requested_reasoning_effort: 'xhigh',
      reasoning_effort: 'max',
      service_tier: 'priority',
      // A conflicting response-side value must not affect the Fast badge.
      actual_service_tier: 'default',
      status: 'failed',
      status_code: 400,
      error_message: 'This content was flagged for possible cybersecurity risk. To get authorized for security work, join the Trusted Access for Cyber program: https://chatgpt.com/cyber',
    })])

    expect(root.textContent).toContain('gpt-5')
    expect(root.textContent).toContain('gpt-5.1')
    expect(root.textContent).toContain('映射模型')
    expect(root.textContent).toContain('响应模型')
    expect(root.textContent).toContain('xhigh -> max')
    expect(root.textContent).toContain('Fast')
    const reasoningBadge = root.querySelector<HTMLElement>('[data-usage-model-badge="reasoning"]')
    const fastBadge = root.querySelector<HTMLElement>('[data-usage-model-badge="fast"]')
    const cyberBadges = root.querySelectorAll<HTMLElement>('[data-usage-model-badge="cyber"]')
    const cyberBadge = cyberBadges[0]
    for (const badge of [reasoningBadge, fastBadge, cyberBadge]) {
      expect(badge).not.toBeNull()
      expect(badge?.classList.contains('h-4')).toBe(true)
      expect(badge?.classList.contains('rounded-full')).toBe(true)
      expect(badge?.classList.contains('px-1.5')).toBe(true)
      expect(badge?.classList.contains('text-[10px]')).toBe(true)
      expect(badge?.classList.contains('leading-4')).toBe(true)
    }
    expect(reasoningBadge?.classList.contains('border-primary/30')).toBe(true)
    expect(reasoningBadge?.classList.contains('bg-primary/5')).toBe(true)
    expect(reasoningBadge?.classList.contains('text-primary')).toBe(true)
    expect(fastBadge?.getAttribute('variant')).toBe('outline-transparent')
    expect(fastBadge?.classList.contains('border-amber-400/50')).toBe(false)
    expect(fastBadge?.classList.contains('!bg-transparent')).toBe(false)
    expect(fastBadge?.classList.contains('bg-amber-400/10')).toBe(false)
    expect(fastBadge?.classList.contains('text-blue-500')).toBe(true)
    expect(cyberBadge?.classList.contains('border-primary/30')).toBe(true)
    expect(cyberBadge?.classList.contains('bg-primary/5')).toBe(true)
    expect(cyberBadge?.classList.contains('text-rose-500')).toBe(true)
    expect(cyberBadges.length).toBeGreaterThan(0)
    expect([...cyberBadges].every(badge => badge.textContent?.trim() === 'Cyber')).toBe(true)
    expect([...cyberBadges].every(badge => badge.title === '上游 Cyber Policy 拒绝')).toBe(true)

    const inlineLayout = root.querySelector('[data-usage-model-layout="inline"]')
    expect(inlineLayout).not.toBeNull()
    const modelRow = inlineLayout?.firstElementChild
    expect(modelRow?.textContent).toContain('gpt-5')
    expect(modelRow?.querySelector('[data-usage-model-mapping]')?.textContent).toContain('映射模型')
    expect(modelRow?.querySelector('[data-usage-model-response]')?.textContent).toContain('响应模型')
    expect(modelRow?.querySelector('[data-usage-model-badge="reasoning"]')?.textContent).toContain('xhigh -> max')
    expect(modelRow?.querySelector('[data-usage-model-badge="fast"]')?.textContent).toContain('Fast')
    expect(modelRow?.querySelector('[data-usage-model-badge="cyber"]')?.textContent).toContain('Cyber')
  })

  it('shows a response model without inventing a mapping arrow or using model_version', () => {
    const root = mountUsageRecordsTable([buildRecord({
      response_model: 'gpt-5.1',
      model_version: 'legacy-version',
      target_model: null,
    })])

    expect(root.querySelector('[data-usage-model-mapping]')).toBeNull()
    expect(root.querySelector('[data-usage-model-response]')?.textContent).toContain('响应模型')
    expect(root.textContent).not.toContain('legacy-version')
    expect(root.textContent).not.toContain('->')
  })

  it('hides a response model when it matches the request model', () => {
    const root = mountUsageRecordsTable([buildRecord({
      response_model: 'gpt-5',
      target_model: null,
    })])

    expect(root.querySelector('[data-usage-model-response]')).toBeNull()
  })

  it('stacks three model badges even without a model mapping', () => {
    const root = mountUsageRecordsTable([buildRecord({
      model: 'gpt-5',
      target_model: null,
      requested_reasoning_effort: 'xhigh',
      reasoning_effort: 'xhigh',
      service_tier: 'priority',
      status: 'failed',
      status_code: 400,
      error_message: 'This content was flagged for possible cybersecurity risk. https://chatgpt.com/cyber',
    })])

    const stackedLayout = root.querySelector('[data-usage-model-layout="stacked"]')
    expect(stackedLayout?.textContent).toContain('gpt-5')
    expect(stackedLayout?.textContent).toContain('xhigh')
    expect(stackedLayout?.textContent).toContain('Fast')
    expect(stackedLayout?.textContent).toContain('Cyber')
  })

  it.each(['priority', 'fast', ' Priority ', 'FAST'])(
    'shows Fast from the final provider request tier %s',
    (requested) => {
      const root = mountUsageRecordsTable([buildRecord({
        service_tier: requested,
        actual_service_tier: 'default',
      })])

      const badge = expectServiceTierBadge(root, 'Fast')
      expect(badge.getAttribute('title')).toBe([
        '上游请求档位：Fast',
        '计费档位：Fast',
      ].join('\n'))
      expect(badge.getAttribute('aria-label')).toBe(
        '上游请求档位：Fast，计费档位：Fast',
      )
      expect(badge.textContent).not.toContain('→')
      expect(badge.textContent).not.toContain('待确认')
      expect(badge.textContent).not.toContain('未确认')
    },
  )

  it.each(['ultrafast', 'flex', 'future-tier', ' UltraFast '])(
    'shows the final provider request tier %s without a fixed badge allowlist',
    (requested) => {
      const root = mountUsageRecordsTable([buildRecord({
        service_tier: requested,
        actual_service_tier: 'default',
      })])
      const badges = [...root.querySelectorAll<HTMLElement>('[data-usage-model-badge="service-tier"]')]
      expect(badges.length).toBeGreaterThan(0)
      for (const badge of badges) {
        expect(badge.textContent?.trim()).toBe(requested.trim())
        expect(badge.title).toBe(`上游请求档位：${requested.trim()}\n计费档位：${requested.trim()}`)
        expect(badge.getAttribute('aria-label')).toBe(
          `上游请求档位：${requested.trim()}，计费档位：${requested.trim()}`,
        )
      }
    },
  )

  it.each(['auto', 'default', 'standard', ' DEFAULT ', '', ' ', null])(
    'does not show a badge for the default or absent request tier %s',
    (requested) => {
      const root = mountUsageRecordsTable([buildRecord({
        service_tier: requested,
        actual_service_tier: 'ultrafast',
      })])
      expect(root.querySelector('[data-usage-model-badge="service-tier"]')).toBeNull()
      expect(root.querySelector('[data-usage-model-badge="fast"]')).toBeNull()
    },
  )

  it.each(['default', 'flex', null])(
    'ignores the response-side tier %s when the request tier is Fast',
    (actualServiceTier) => {
      const root = mountUsageRecordsTable([buildRecord({
        service_tier: 'priority',
        actual_service_tier: actualServiceTier,
      })])

      expectServiceTierBadge(root, 'Fast')
      expect(root.textContent).not.toContain('Fast →')
    },
  )

  it('does not infer Fast from a response-side priority tier', () => {
    const root = mountUsageRecordsTable([buildRecord({
      service_tier: 'default',
      actual_service_tier: 'priority',
    })])

    expect(root.querySelector('[data-usage-model-badge="fast"]')).toBeNull()
  })

  it('does not infer Fast when only the response has a tier', () => {
    const root = mountUsageRecordsTable([buildRecord({
      service_tier: null,
      actual_service_tier: 'priority',
    })])

    expect(root.querySelector('[data-usage-model-badge="fast"]')).toBeNull()
  })

  it.each(['pending', 'streaming'] as const)(
    'keeps Fast stable while a priority request is %s',
    (status) => {
      const root = mountUsageRecordsTable([buildRecord({
        service_tier: 'priority',
        actual_service_tier: null,
        status,
      })])

      expectServiceTierBadge(root, 'Fast')
    },
  )

  it('keeps Fast stable for a completed request without a response tier', () => {
    const root = mountUsageRecordsTable([buildRecord({
      service_tier: 'priority',
      actual_service_tier: null,
      status: 'completed',
    })])

    expectServiceTierBadge(root, 'Fast')
  })

  it('offers embedding API formats in the usage record filter', () => {
    const root = mountUsageRecordsTable([buildRecord({ api_format: 'openai:chat' })])

    expect(root.textContent).toContain('OpenAI Embedding')
    expect(root.textContent).toContain('Gemini Embedding')
    expect(root.textContent).toContain('Jina Embedding')
    expect(root.textContent).toContain('Doubao Embedding')
    const liveLabels = [...root.querySelectorAll<HTMLElement>('div')]
      .filter(element => element.textContent?.trim() === 'OpenAI Live')
    expect(liveLabels.length).toBeGreaterThanOrEqual(2)
    expect(root.querySelector('[data-filter-value="codex:live"]')).not.toBeNull()
  })

  it('does not expose the canonical Codex Live id in row labels or tooltips', () => {
    const root = mountUsageRecordsTable([buildRecord({ api_format: 'codex:live' })])

    expect(root.textContent).toContain('OpenAI Live')
    expect(root.textContent).not.toContain('codex:live')
    expect(root.querySelector('[title="OpenAI Live"]')).not.toBeNull()
  })

  it('emits hide unknown toggle changes', () => {
    const onUpdateHideUnknownRecords = vi.fn()
    const root = mountUsageRecordsTable([buildRecord()], {
      'onUpdate:hideUnknownRecords': onUpdateHideUnknownRecords,
    })

    root.querySelector<HTMLElement>('[data-usage-hide-unknown-toggle="desktop"]')?.click()

    expect(onUpdateHideUnknownRecords).toHaveBeenCalledWith(true)
  })

  it('debounces usage search updates', async () => {
    vi.useFakeTimers()
    const onUpdateFilterSearch = vi.fn()
    const root = mountUsageRecordsTable([buildRecord()], {
      'onUpdate:filterSearch': onUpdateFilterSearch,
    })
    const input = root.querySelector<HTMLInputElement>('#usage-records-search')
    expect(input).not.toBeNull()

    input!.value = 'a'
    input!.dispatchEvent(new Event('input'))
    input!.value = 'ab'
    input!.dispatchEvent(new Event('input'))
    input!.value = 'abc'
    input!.dispatchEvent(new Event('input'))
    await nextTick()

    await vi.advanceTimersByTimeAsync(299)
    expect(onUpdateFilterSearch).not.toHaveBeenCalled()

    await vi.advanceTimersByTimeAsync(1)
    expect(onUpdateFilterSearch).toHaveBeenCalledTimes(1)
    expect(onUpdateFilterSearch).toHaveBeenCalledWith('abc')
  })

  it('shows retry and fallback markers together when both flags are set', () => {
    const root = mountUsageRecordsTable([buildRecord({
      has_fallback: true,
      has_retry: true,
    })])

    expect(root.querySelector('[data-usage-attempt-marker="fallback"]')).not.toBeNull()
    expect(root.querySelector('[data-usage-attempt-marker="retry"]')).not.toBeNull()
  })

  it('shows the PII redaction marker in both admin and user layouts', () => {
    const adminRoot = mountUsageRecordsTable([buildRecord({ chat_pii_redaction: true })])
    const adminMarkers = [...adminRoot.querySelectorAll('[data-usage-attempt-marker="pii-redaction"]')]
    expect(adminMarkers).toHaveLength(2)
    for (const marker of adminMarkers) {
      expect(marker.getAttribute('aria-label')).toBe('触发敏感信息保护')
      expect(marker.getAttribute('title')).toContain('上游')
    }

    const userRoot = mountUsageRecordsTable(
      [buildRecord({ chat_pii_redaction: true })],
      { isAdmin: false },
    )
    expect([...userRoot.querySelectorAll('[data-usage-attempt-marker="pii-redaction"]')]).toHaveLength(2)
  })

  it('hides the PII redaction marker when the request was never redacted', () => {
    const root = mountUsageRecordsTable([buildRecord({ chat_pii_redaction: false })])
    expect(root.querySelector('[data-usage-attempt-marker="pii-redaction"]')).toBeNull()
  })

  it('shows the skipped-candidate marker when a candidate was skipped by scheduling', () => {
    const root = mountUsageRecordsTable([buildRecord({
      has_skipped_candidate: true,
      skipped_candidate_reasons: ['key_rpm_exhausted'],
    })])

    const marker = root.querySelector('[data-usage-attempt-marker="skipped-candidate"]')
    expect(marker).not.toBeNull()
    // tooltip 必须说明"请求未发往该候选"，否则用户会以为上游报了错
    const title = marker?.getAttribute('title') ?? ''
    expect(title).toContain('调度阶段被跳过')
    expect(title).toContain('不会有上游报错')
    expect(title).toContain('密钥本分钟请求数已达上限')
  })

  it('prefers the fallback marker over the skipped-candidate marker', () => {
    // 真正发生过故障转移时，琥珀色转移图标信息量更大，不再叠加灰色角标
    const root = mountUsageRecordsTable([buildRecord({
      has_fallback: true,
      has_skipped_candidate: true,
      skipped_candidate_reasons: ['key_rpm_exhausted'],
    })])

    expect(root.querySelector('[data-usage-attempt-marker="fallback"]')).not.toBeNull()
    expect(root.querySelector('[data-usage-attempt-marker="skipped-candidate"]')).toBeNull()
  })

  it('hides the skipped-candidate marker when no candidate was skipped', () => {
    const root = mountUsageRecordsTable([buildRecord({ has_skipped_candidate: false })])

    expect(root.querySelector('[data-usage-attempt-marker="skipped-candidate"]')).toBeNull()
  })
})
