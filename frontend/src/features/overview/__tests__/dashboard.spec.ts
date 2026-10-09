import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { createApp, nextTick, type App } from 'vue'
import { createPinia, disposePinia, type Pinia } from 'pinia'
import { createMemoryHistory, createRouter } from 'vue-router'
import AdminDashboard from '@/views/admin/AdminDashboard.vue'
import type { OverviewDashboardCharts, OverviewDashboardChartMetrics } from '@/api/overview'
import type { DateRangeParams } from '@/features/usage/types'
import { setI18nLocale } from '@/i18n'
import { dashboardChartRange, dayRange, modelDatasets, providerSlices } from '../dashboard/charts'
import { dashboardSummary } from './fixtures/dashboardSummary'
import { useAnnouncementStore } from '@/stores/announcements'

const api = vi.hoisted(() => ({ dashboardSummary: vi.fn(), summary: vi.fn(), dashboardTotal: vi.fn(), dashboardCharts: vi.fn() }))
const legacyApi = vi.hoisted(() => ({ getStats: vi.fn(), getDailyStats: vi.fn() }))
const announcementApi = vi.hoisted(() => ({ getAnnouncements: vi.fn(), getUserAnnouncements: vi.fn(), markAsRead: vi.fn() }))
const auth = vi.hoisted(() => ({ canAccessAdmin: true, isAdmin: true, isAuditAdmin: false }))
const picker = vi.hoisted(() => ({ next: null as DateRangeParams | null }))
vi.mock('@/api/overview', () => ({ overviewApi: api }))
vi.mock('@/api/dashboard', () => ({ dashboardApi: legacyApi }))
vi.mock('@/features/usage/components/IntervalTimelineCard.vue', async () => { const { h, defineComponent } = await import('vue'); return { default: defineComponent({ props: { title: String, hours: Number, isAdmin: Boolean, refreshIntervalMs: Number }, setup: props => () => h('div', { 'data-timeline': '' }, props.title) }) } })
vi.mock('@/stores/auth', () => ({ useAuthStore: () => auth }))
vi.mock('@/api/announcements', () => ({ announcementApi }))
vi.mock('@/components/charts/BarChart.vue', async () => { const { h, defineComponent } = await import('vue'); return { default: defineComponent({ props: { data: Object }, setup: props => () => h('div', { 'data-chart': 'bar', 'data-values': JSON.stringify(props.data) }) }) } })
vi.mock('@/components/charts/DoughnutChart.vue', async () => { const { h, defineComponent } = await import('vue'); return { default: defineComponent({ props: { data: Object }, setup: props => () => h('div', { 'data-chart': 'doughnut', 'data-values': JSON.stringify(props.data) }) }) } })
vi.mock('@/components/charts/LineChart.vue', async () => { const { h } = await import('vue'); return { default: { render: () => h('div', { 'data-chart': 'line' }) } } })
vi.mock('@/components/common', async () => {
  const { defineComponent, h } = await import('vue')
  return { TimeRangePicker: defineComponent({
    props: { modelValue: { type: Object, default: () => ({}) } }, emits: ['update:modelValue'],
    setup: (props, { emit }) => () => h('button', { 'data-period-control': '', onClick: () => emit('update:modelValue', picker.next ?? { ...props.modelValue, preset: 'last30days' }) }, 'Last 30 days'),
  }) }
})
vi.mock('@/components/ui', async () => {
  const { defineComponent, h } = await import('vue')
  const passthrough = (name: string, tag = 'div') => defineComponent({ name, setup: (_, { slots }) => () => h(tag, slots.default?.()) })
  return {
    Card: passthrough('CardStub', 'section'), Badge: passthrough('BadgeStub', 'span'), Button: passthrough('ButtonStub', 'button'),
    Skeleton: passthrough('SkeletonStub'),
    Table: passthrough('TableStub', 'table'), TableHeader: passthrough('TableHeaderStub', 'thead'), TableBody: passthrough('TableBodyStub', 'tbody'), TableRow: passthrough('TableRowStub', 'tr'), TableHead: passthrough('TableHeadStub', 'th'), TableCell: passthrough('TableCellStub', 'td'),
  }
})

const amount = (value: string | null, status = value === null ? 'unknown' : 'known') => ({ value, status, currency: 'USD', basis: 'billable' })
const metrics = (request_count = 0): OverviewDashboardChartMetrics => ({
  request_count, successful_request_count: request_count, failed_request_count: 0, cancelled_request_count: 0, in_flight_request_count: 0, unclassified_failure_count: null,
  input_tokens: null, output_tokens: null, total_tokens: request_count * 20, usage_active_users: null, slow_request_count: null, enabled_users: 8,
  success_rate: { value: request_count ? 1 : null, numerator: request_count, denominator: request_count }, latency_ms: { avg: 100, p50: 90, p95: 190, p99: 230, sample_count: request_count },
  rated_amount: amount('10'), billable_amount: amount('5'), quota_covered_amount: amount('2'), wallet_consumed_amount: amount('3'), wallet_debit_amount: amount('3'),
})
const charts = (): OverviewDashboardCharts => ({
  summary: metrics(777), series: [{ ...metrics(9), bucket_start: '2026-09-05T00:00:00Z' }],
  models: [{ ...metrics(9), id: 'gpt-example', label: 'gpt-example', bucket_start: '2026-09-05T00:00:00Z' }], providers: [{ ...metrics(9), id: 'provider-1', label: 'Provider 1' }],
})
const mounted: { app: App; root: HTMLElement; pinia: Pinia }[] = []
async function settle() { for (let i = 0; i < 12; i++) { await Promise.resolve(); await nextTick() } await new Promise(resolve => setTimeout(resolve, 0)) }
async function mount() {
  const root = document.createElement('div')
  document.body.appendChild(root)
  const app = createApp(AdminDashboard)
  const pinia = createPinia()
  const router = createRouter({
    history: createMemoryHistory(),
    routes: [
      { path: '/', component: AdminDashboard },
      { path: '/admin/announcements', component: { render: () => null } },
    ],
  })
  app.use(pinia)
  app.use(router)
  useAnnouncementStore(pinia).resetSession('dashboard-admin')
  mounted.push({ app, root, pinia })
  await router.isReady()
  app.mount(root)
  await settle()
  return { root }
}

function card(root: HTMLElement, label: string) {
  return Array.from(root.querySelectorAll('section')).find(section => Array.from(section.querySelectorAll('p')).some(p => p.textContent?.trim() === label))
}

function primaryValue(root: HTMLElement, label: string) {
  return card(root, label)?.querySelector('p')?.nextElementSibling as HTMLElement | null | undefined
}

beforeEach(() => {
  vi.stubGlobal('matchMedia', vi.fn(() => ({ matches: true, addEventListener: vi.fn(), removeEventListener: vi.fn() })))
  setI18nLocale('zh-CN')
  vi.resetAllMocks()
  picker.next = null
  Object.assign(auth, { canAccessAdmin: true, isAdmin: true, isAuditAdmin: false })
  api.dashboardSummary.mockResolvedValue(dashboardSummary())
  api.dashboardCharts.mockResolvedValue({ data: charts() })
  announcementApi.getUserAnnouncements.mockResolvedValue({ items: [], total: 0, unread_count: 0 })
  legacyApi.getStats.mockResolvedValue({
    stats: [{ name: 'Legacy request metric', value: '999999', icon: 'Activity' }],
    system_health: { avg_response_time: 1.5, error_rate: 2, error_requests: 1, fallback_count: 3, total_requests: 50 },
    cost_stats: { total_cost: 42.5, total_actual_cost: 35, cost_savings: 7.5 },
    cache_stats: { cache_creation_tokens: 10, cache_read_tokens: 20, total_cache_tokens: 30 },
  })
  legacyApi.getDailyStats.mockResolvedValue({
    daily_stats: [{ date: '2026-09-05', requests: 777, tokens: 8888, cost: 3.5, avg_response_time: 1.2, unique_models: 1, unique_providers: 1, model_breakdown: [{ model: 'gpt-example', cost: 3.5 }] }],
    provider_summary: [{ provider: 'Primary', requests: 777, tokens: 8888, cost: 3.5 }], model_summary: [], period: { start_date: '2026-09-05', end_date: '2026-09-11', days: 7 },
  })
})
afterEach(() => { for (const { app, root, pinia } of mounted.splice(0)) { app.unmount(); disposePinia(pinia); root.remove() } vi.unstubAllGlobals() })

function metric(root: HTMLElement, key: string) {
  return root.querySelector(`[data-request-metric="${key}"]`)
}

function metricValue(root: HTMLElement, key: string) {
  return metric(root, key)?.lastElementChild?.textContent?.trim()
}

describe('administrator dashboard compact presentation', () => {
  it('shows five primary cards, six request metrics, announcements, and the activity streak', async () => {
    const { root } = await mount()
    expect(card(root, '今日请求')?.textContent).toContain('103')
    expect(card(root, '今日请求')?.textContent).toContain('总请求 12,345')
    const tokens = card(root, '今日 Token')
    expect(tokens?.textContent).toContain('12K / 3.46K')
    expect(tokens?.textContent).toContain('总 Token 1.23M')
    const cache = card(root, '今日缓存')
    expect(tokens?.nextElementSibling).toBe(cache)
    expect(cache?.textContent).toContain('30.00%')
    expect(cache?.textContent).toContain('总缓存 45.00%')
    expect(card(root, '今日消费')?.textContent).toContain('$9.87')
    expect(card(root, '今日消费')?.textContent).toContain('总消费 $98.76')
    const users = card(root, '今日活跃用户')
    expect(users?.textContent).toContain('总用户 14')
    expect(users?.textContent).toContain('+2')
    expect(users?.textContent).toContain('-1')
    expect(metric(root, 'first-byte')?.textContent).toContain('80ms')
    expect(metric(root, 'response')?.textContent).toContain('1.52s')
    expect(Array.from(root.querySelectorAll('[data-request-metric]'), item => item.getAttribute('data-request-metric')))
      .toEqual(['first-byte', 'response', 'concurrency-avg', 'concurrency-peak', 'stream', 'standard'])
    expect(metric(root, 'cache-hit')).toBeNull()
    expect(metric(root, 'cache-tokens')).toBeNull()
    expect(metric(root, 'concurrency-avg')?.textContent).toContain('2.5')
    expect(metric(root, 'concurrency-peak')?.textContent).toContain('8')
    expect(metric(root, 'stream')?.textContent).toContain('90')
    expect(metric(root, 'standard')?.textContent).toContain('13')
    const activity = root.querySelector('[data-dashboard-activity]')
    expect(activity?.textContent).toContain('连续活跃')
    expect(activity?.textContent).toContain('2 / 413')
    expect(root.textContent?.indexOf('连续活跃')).toBeLessThan(root.textContent?.indexOf('统计周期') ?? -1)
    expect(root.querySelector('[data-timeline]')?.textContent).toContain('最近24小时')
    expect(root.textContent).not.toContain('P95')
    expect(root.textContent).not.toContain('P99')
    expect(root.textContent).not.toContain('成功率')
    expect(root.textContent).not.toContain('RPM')
    expect(root.textContent).not.toContain('TPM')
    expect(root.textContent).not.toContain('统计自')
    expect(root.textContent).not.toContain('累计统计时间')
    expect(card(root, '今日请求')?.querySelector('svg')).toBeNull()
    expect(metric(root, 'first-byte')?.closest('section')?.querySelector('svg')).toBeNull()
    expect(api.dashboardSummary).toHaveBeenCalledTimes(1)
    expect(api.summary).not.toHaveBeenCalled()
    expect(api.dashboardTotal).not.toHaveBeenCalled()
    expect(api.dashboardCharts).toHaveBeenCalledTimes(1)
    expect(api.dashboardCharts).toHaveBeenCalledWith(expect.objectContaining({ timezone: Intl.DateTimeFormat().resolvedOptions().timeZone, from: expect.any(String), to: expect.any(String) }), expect.any(AbortSignal))
    expect(legacyApi.getStats).not.toHaveBeenCalled()
    expect(legacyApi.getDailyStats).not.toHaveBeenCalled()
    expect(announcementApi.getAnnouncements).not.toHaveBeenCalled()
    expect(announcementApi.getUserAnnouncements).toHaveBeenCalledExactlyOnceWith({ limit: 20, offset: 0 })
    expect(root.querySelector('#announcements-section')?.textContent).toContain('系统公告')
    expect(root.querySelector('#announcements-section a')?.getAttribute('href')).toBe('/admin/announcements')
  })

  it('keeps today and all-period totals fixed when the chart period changes', async () => {
    const { root } = await mount()
    root.querySelector<HTMLButtonElement>('[data-period-control]')!.click()
    await new Promise(resolve => setTimeout(resolve, 150))
    await settle()
    expect(api.dashboardCharts).toHaveBeenCalledTimes(2)
    expect(api.dashboardCharts.mock.calls[1]![0].from).not.toBe(api.dashboardCharts.mock.calls[0]![0].from)
    expect(legacyApi.getDailyStats).not.toHaveBeenCalled()
    expect(api.dashboardSummary).toHaveBeenCalledTimes(1)
    expect(card(root, '今日请求')?.textContent).toContain('12,345')
  })

  it('uses the same snapshot for audit administrators', async () => {
    Object.assign(auth, { isAdmin: false, isAuditAdmin: true, canAccessAdmin: true })
    const { root } = await mount()
    expect(root.textContent).toContain('AUDIT MODE')
    expect(card(root, '今日请求')?.textContent).toContain('12,345')
    expect(api.dashboardSummary).toHaveBeenCalledTimes(1)
    expect(legacyApi.getStats).not.toHaveBeenCalled()
  })

  it('keeps unavailable values unknown and does not invent cache hit rate from output tokens', async () => {
    const snapshot = dashboardSummary()
    Object.assign(snapshot.today, { input_tokens: null, output_tokens: null, cache_read_tokens: 0, cache_input_tokens: 0, avg_first_byte_ms: null, avg_response_ms: null, billable_amount: { value: null, status: 'unknown', currency: 'USD', basis: 'billable' } })
    Object.assign(snapshot.total, { cache_read_tokens: null, cache_input_tokens: null })
    snapshot.concurrency = { avg: null, peak: null, observed_from: null, observed_through: null, scope: 'node', coverage: 'unavailable' }
    api.dashboardSummary.mockResolvedValue(snapshot)
    const { root } = await mount()
    for (const key of ['first-byte', 'response', 'concurrency-avg', 'concurrency-peak']) {
      expect(metricValue(root, key)).toBe('—')
    }
    expect(primaryValue(root, '今日 Token')?.textContent?.trim()).toBe('— / —')
    expect(primaryValue(root, '今日消费')?.textContent?.trim()).toBe('—')
    expect(primaryValue(root, '今日缓存')?.textContent?.trim()).toBe('—')
    expect(card(root, '今日缓存')?.textContent).toContain('总缓存 —')
    expect(card(root, '今日缓存')?.textContent).not.toContain('0.00%')
    expect(root.querySelector('[role="alert"]')).toBeNull()
  })

  it('puts collection boundaries, partial prices, and node observation coverage in tooltips only', async () => {
    const snapshot = dashboardSummary()
    snapshot.today.billable_amount.status = 'known_subtotal'
    api.dashboardSummary.mockResolvedValue(snapshot)
    const { root } = await mount()
    expect(card(root, '今日请求')?.querySelector('p')?.title).toContain('统计自')
    expect(primaryValue(root, '今日消费')?.querySelector('[title]')?.getAttribute('title')).toContain('部分请求价格未知')
    const concurrency = metric(root, 'concurrency-avg')?.closest('section')
    expect(concurrency?.title).toContain('当前节点')
    expect(concurrency?.title).toContain('部分时段')
    expect(concurrency?.title).toContain('02:00')
    expect(concurrency?.title).toContain('04:00')
    expect(root.textContent).not.toContain('已知小计')
    expect(root.textContent).not.toContain('部分时段')
    expect(root.textContent).not.toContain('统计自')
    const heatmapDay = root.querySelector<HTMLElement>('[title*="250"]')
    expect(heatmapDay?.title).not.toContain('$0')
    expect(heatmapDay?.title).not.toContain('tokens')
  })

  it('leaves primary and request values blank on failure without error text or a retry button', async () => {
    api.dashboardSummary.mockRejectedValue(new Error('Unavailable'))
    const { root } = await mount()
    expect(root.querySelector('[role="alert"]')).toBeNull()
    expect(root.textContent).not.toContain('加载失败')
    expect(root.textContent).not.toContain('重试')
    expect(card(root, '今日请求')?.textContent).toContain('—')
    for (const key of ['first-byte', 'response', 'concurrency-avg', 'concurrency-peak', 'stream', 'standard']) {
      expect(metricValue(root, key)).toBe('—')
    }
    expect(root.querySelector('[data-chart="bar"]')).not.toBeNull()
  })

  it('uses customer billing and all request types for daily rows, totals, and both cost charts', async () => {
    const snapshot = dashboardSummary()
    Object.assign(snapshot.today, { request_count: 25018, stream_requests: 24989, standard_requests: 29, billable_amount: amount('10855.49') })
    api.dashboardSummary.mockResolvedValue(snapshot)
    const metric = { ...metrics(25018), billable_amount: amount('10855.49'), rated_amount: amount('13580.42'), latency_ms: { ...metrics().latency_ms, avg: 1520 } }
    const data: OverviewDashboardCharts = {
      summary: metric,
      series: [{ ...metric, bucket_start: '2026-09-19T00:00:00Z', unique_providers: 2 }],
      models: [{ ...metric, id: 'model', label: 'Model', bucket_start: '2026-09-19T00:00:00+00:00' }],
      providers: [{ ...metric, id: 'provider', label: 'Provider' }],
    }
    api.dashboardCharts.mockResolvedValue({ data })
    const { root } = await mount()
    const row = root.querySelector('[data-daily-date]')
    expect(row?.textContent).toContain('25,018')
    expect(row?.textContent).toContain('$10855.4900')
    expect(row?.textContent).toContain('1.52s')
    expect(row?.lastElementChild?.textContent?.trim()).toBe('2')
    expect(primaryValue(root, '今日消费')?.textContent).toContain('$10855.49')
    expect(metricValue(root, 'stream')).toBe('24,989')
    expect(metricValue(root, 'standard')).toBe('29')
    expect(root.querySelector('[data-daily-total]')?.textContent).toContain('$10855.4900')
    for (const kind of ['bar', 'doughnut']) {
      const chart = JSON.parse(root.querySelector(`[data-chart="${kind}"]`)!.getAttribute('data-values')!)
      expect(chart.datasets[0].data).toEqual([10855.49])
    }
    expect(root.textContent).not.toContain('13580.42')
    expect(legacyApi.getDailyStats).not.toHaveBeenCalled()
  })

  it.each([
    { value: null, status: 'unknown', display: '—' },
    { value: '123', status: 'unknown', display: '—' },
    { value: '0', status: 'known', display: '$0.0000' },
    { value: '0', status: 'known_subtotal', display: '$0.0000' },
    { value: '3.25', status: 'known_subtotal', display: '$3.2500' },
  ])('preserves daily amount $status/$value without inventing zero', async ({ value, status, display }) => {
    const data = charts()
    const billable = amount(value, status)
    for (const metric of [data.summary, ...data.series, ...data.models, ...data.providers]) metric.billable_amount = billable
    data.series[0]!.total_tokens = null
    data.series[0]!.latency_ms.avg = null
    api.dashboardCharts.mockResolvedValue({ data })
    const { root } = await mount()
    const cells = root.querySelector('[data-daily-date]')!.querySelectorAll('td')
    expect(cells[2]!.textContent?.trim()).toBe('—')
    expect(cells[3]!.textContent).toContain(display)
    expect(cells[4]!.textContent?.trim()).toBe('—')
    expect(cells[6]!.textContent?.trim()).toBe('—')
    expect(root.querySelector('[data-daily-total]')?.textContent).toContain(display)
    if (status === 'unknown') {
      expect(cells[3]!.textContent).not.toContain('$')
      expect(root.querySelector('[data-chart="bar"]')).toBeNull()
      expect(root.querySelector('[data-chart="doughnut"]')).toBeNull()
      expect(root.textContent).toContain('费用尚未确认')
    }
    if (status === 'known_subtotal') {
      expect(cells[3]!.textContent).toContain('已知小计')
      expect(root.querySelector('[data-daily-total]')?.textContent).toContain('已知小计')
      expect(root.textContent).toContain('分布占比按已知金额计算')
      if (value === '0') {
        expect(root.textContent).toContain('费用尚未确认')
        expect(root.textContent).not.toContain('此周期暂无计费费用')
      }
    }
  })

  it('takes period totals and latency from the canonical summary instead of averaging daily values', async () => {
    const data = charts()
    data.summary = { ...metrics(101), billable_amount: amount('12', 'known_subtotal'), latency_ms: { ...metrics().latency_ms, avg: 500 } }
    data.series = [
      { ...metrics(100), bucket_start: '2026-09-05T00:00:00Z', latency_ms: { ...metrics().latency_ms, avg: 100, sample_count: 1 } },
      { ...metrics(1), bucket_start: '2026-09-06T00:00:00Z', latency_ms: { ...metrics().latency_ms, avg: 900, sample_count: 1 } },
    ]
    api.dashboardCharts.mockResolvedValue({ data })
    const { root } = await mount()
    const total = root.querySelector('[data-daily-total]')
    expect(total?.textContent).toContain('$12.0000')
    expect(total?.textContent).toContain('500ms')
    expect(total?.textContent).toContain('已知小计')
  })

  it('does not describe a positive summary with missing chart details as zero cost', async () => {
    const data = charts()
    data.models = []
    data.providers = []
    api.dashboardCharts.mockResolvedValue({ data })
    const { root } = await mount()
    expect(root.querySelector('[data-daily-date]')?.textContent).toContain('$5.0000')
    expect(root.textContent).toContain('暂无费用明细')
    expect(root.textContent).not.toContain('此周期暂无计费费用')
  })

  it('hides the previous period total while loading the selected range', async () => {
    const { root } = await mount()
    expect(root.querySelector('[data-daily-total]')).not.toBeNull()
    api.dashboardCharts.mockReturnValue(new Promise(() => {}))
    root.querySelector<HTMLButtonElement>('[data-period-control]')!.click()
    await nextTick()
    expect(root.querySelector('[data-daily-total]')).toBeNull()
    await new Promise(resolve => setTimeout(resolve, 150))
    await settle()
    expect(root.querySelector('[data-daily-total]')).toBeNull()
  })

  it('cancels a previous chart range and ignores its late response after the new range has loaded', async () => {
    let resolveOld!: (value: { data: OverviewDashboardCharts }) => void
    api.dashboardCharts.mockImplementationOnce(() => new Promise(resolve => { resolveOld = resolve }))
    const { root } = await mount()
    const oldSignal = api.dashboardCharts.mock.calls[0]![1] as AbortSignal
    const current = charts()
    current.series[0]!.billable_amount = amount('42')
    api.dashboardCharts.mockResolvedValue({ data: current })
    root.querySelector<HTMLButtonElement>('[data-period-control]')!.click()
    await nextTick()
    expect(oldSignal.aborted).toBe(true)
    await new Promise(resolve => setTimeout(resolve, 150))
    await settle()
    expect(root.querySelector('[data-daily-date]')?.textContent).toContain('$42.0000')
    resolveOld({ data: charts() })
    await settle()
    expect(root.querySelector('[data-daily-date]')?.textContent).toContain('$42.0000')
    expect(api.dashboardCharts).toHaveBeenCalledTimes(2)
  })

  it('shows a retryable daily load error without replacing the summary or inventing empty statistics', async () => {
    api.dashboardCharts.mockRejectedValueOnce(new Error('timeout'))
    const { root } = await mount()
    const alert = root.querySelector('[role="alert"]')
    expect(alert?.textContent).toContain('统计加载失败，请重试')
    expect(primaryValue(root, '今日消费')?.textContent).toContain('$9.87')
    expect(root.querySelector('[data-daily-date]')).toBeNull()
    expect(root.querySelector('[data-daily-total]')).toBeNull()
    expect(root.textContent).not.toContain('暂无数据')
    alert!.querySelector<HTMLButtonElement>('button')!.click()
    await settle()
    expect(root.querySelector('[role="alert"]')).toBeNull()
    expect(root.querySelector('[data-daily-date]')?.textContent).toContain('$5.0000')
    expect(api.dashboardCharts).toHaveBeenCalledTimes(2)
    expect(api.dashboardSummary).toHaveBeenCalledTimes(1)
  })

  it('uses the selected calendar timezone for chart requests and daily date labels', async () => {
    const { root } = await mount()
    picker.next = { start_date: '2026-03-08', end_date: '2026-03-08', timezone: 'America/New_York' }
    const data = charts()
    data.series[0]!.bucket_start = '2026-03-08T05:00:00Z'
    api.dashboardCharts.mockResolvedValue({ data })
    root.querySelector<HTMLButtonElement>('[data-period-control]')!.click()
    await new Promise(resolve => setTimeout(resolve, 150))
    await settle()
    expect(api.dashboardCharts).toHaveBeenLastCalledWith({ from: '2026-03-08T05:00:00.000Z', to: '2026-03-09T04:00:00.000Z', timezone: 'America/New_York' }, expect.any(AbortSignal))
    expect(api.dashboardSummary).toHaveBeenLastCalledWith('America/New_York', expect.any(AbortSignal))
    expect(root.querySelector('[data-daily-date]')?.firstElementChild?.textContent).toContain('03/08')
  })
})

describe('dashboard chart accounting', () => {
  it('preserves inclusive calendar periods and explicit instants in the selected timezone', () => {
    const now = new Date('2026-10-08T04:34:56Z')
    expect(dashboardChartRange({ preset: 'last7days', timezone: 'Asia/Shanghai' }, now)).toEqual({ from: '2026-10-01T16:00:00.000Z', to: '2026-10-08T16:00:00.000Z', timezone: 'Asia/Shanghai' })
    expect(dashboardChartRange({ preset: 'yesterday', timezone: 'Asia/Shanghai' }, now)).toEqual({ from: '2026-10-06T16:00:00.000Z', to: '2026-10-07T16:00:00.000Z', timezone: 'Asia/Shanghai' })
    expect(dashboardChartRange({ start_date: '2026-03-08', end_date: '2026-03-08', timezone: 'America/New_York' }, now)).toEqual({ from: '2026-03-08T05:00:00.000Z', to: '2026-03-09T04:00:00.000Z', timezone: 'America/New_York' })
    expect(dashboardChartRange({ start_date: '2026-09-06', end_date: '2026-09-06', timezone: 'America/Santiago' }, now)).toEqual({ from: '2026-09-06T04:00:00.000Z', to: '2026-09-07T03:00:00.000Z', timezone: 'America/Santiago' })
    expect(dashboardChartRange({ from: '2026-10-07T16:12:34Z', to: '2026-10-08T16:56:00Z', timezone: 'Asia/Shanghai' }, now)).toEqual({ from: '2026-10-07T16:12:34.000Z', to: '2026-10-08T16:56:00.000Z', timezone: 'Asia/Shanghai' })
  })
  it('preserves the full provider sum when collapsing additional groups and excludes unknown amounts', () => {
    const rows = Array.from({ length: 105 }, (_, i) => ({ ...metrics(1), id: String(i), label: `Provider ${i}`, billable_amount: amount(String(i + 1)) }))
    rows.push({ ...metrics(1), id: 'unknown', label: 'Unknown cost', billable_amount: amount(null) })
    const slices = providerSlices(rows, 'Unknown provider', 'Others')
    expect(slices).toHaveLength(7)
    expect(slices.reduce((sum, slice) => sum + slice.value, 0)).toBe(5565)
    expect(slices.some(slice => slice.label === 'Unknown cost')).toBe(false)
  })

  it('keeps unknown model amounts null and does not use period percentages to distribute daily amounts', () => {
    const data = charts()
    data.series.push({ ...metrics(1), bucket_start: '2026-09-06T00:00:00Z' })
    data.models = [
      { ...metrics(1), id: 'gpt-example', label: 'gpt-example', bucket_start: '2026-09-05T00:00:00+00:00', billable_amount: amount('3') },
      { ...metrics(1), id: 'gpt-example', label: 'gpt-example', bucket_start: data.series[1]!.bucket_start, billable_amount: amount(null) },
    ]
    expect(modelDatasets(data, 'Unknown model', 'Others')[0]).toMatchObject({ label: 'gpt-example', data: [3, null] })
  })

  it.each(['unknown', 'known_subtotal', 'estimated_subtotal'])('keeps absent model rows unknown in an incomplete %s day', status => {
    const data = charts()
    data.series.push({ ...metrics(0), billable_amount: amount(status === 'unknown' ? null : '0', status), bucket_start: '2026-09-06T00:00:00Z' })
    data.series.push({ ...metrics(0), billable_amount: amount('0'), bucket_start: '2026-09-07T00:00:00Z' })
    expect(modelDatasets(data, 'Unknown model', 'Others')[0]!.data).toEqual([5, null, 0])
  })

  it('uses a 23-hour local day during daylight saving and clips it to report boundaries', () => {
    const dstRange = { from: '2026-03-07T05:00:00Z', to: '2026-03-10T04:00:00Z', timezone: 'America/New_York' }
    expect(dayRange('2026-03-08T05:00:00Z', dstRange)).toEqual({ from: '2026-03-08T05:00:00.000Z', to: '2026-03-09T04:00:00.000Z', timezone: dstRange.timezone })
    expect(dayRange('2026-03-08T05:00:00Z', { ...dstRange, to: '2026-03-08T18:00:00Z' }).to).toBe('2026-03-08T18:00:00.000Z')
  })

  it('uses the first valid local minute when a timezone skips midnight', () => {
    const santiago = { from: '2026-09-05T04:00:00Z', to: '2026-09-08T03:00:00Z', timezone: 'America/Santiago' }
    expect(dayRange('2026-09-05T04:00:00Z', santiago).to).toBe('2026-09-06T04:00:00.000Z')
  })
})
