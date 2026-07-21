import { afterEach, describe, expect, it, vi } from 'vitest'
import { createApp, defineComponent, h, nextTick, type App } from 'vue'
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
    TableFilterMenu: passthrough('TableFilterMenuStub'),
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
      '首字: 0.50s',
      '总耗时: 1.00s',
      '生成耗时: 0.50s',
      '输出速度: 100 tokens/s',
    ].join('\n'))
    expect(titles.join('\n')).not.toContain('500ms')
    expect(titles.join('\n')).not.toContain('首字后生成耗时')
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
      '首字: 0.50s',
      '总耗时: 1.00s',
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

  it('shows mapping, reasoning, Fast, and Cyber in the model area', () => {
    const root = mountUsageRecordsTable([buildRecord({
      model: 'gpt-5',
      target_model: 'gpt-5.1',
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
    expect(modelRow?.textContent).toContain('->')
    expect(modelRow?.textContent).toContain('gpt-5.1')
    expect(modelRow?.querySelector('[data-usage-model-target]')?.classList.contains('basis-full')).toBe(true)
    expect(modelRow?.querySelector('[data-usage-model-target]')?.classList.contains('order-last')).toBe(true)
    expect(modelRow?.querySelector('[data-usage-model-badge="reasoning"]')?.textContent).toContain('xhigh -> max')
    expect(modelRow?.querySelector('[data-usage-model-badge="fast"]')?.textContent).toContain('Fast')
    expect(modelRow?.querySelector('[data-usage-model-badge="cyber"]')?.textContent).toContain('Cyber')
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
})
