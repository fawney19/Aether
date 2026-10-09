import { createApp, nextTick, type App } from 'vue'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import type { OverviewDashboardCharts, OverviewDashboardSummary, OverviewDashboardChartMetrics, OverviewRange, OverviewResponse } from '@/api/overview'
import { dashboardSummary } from '@/features/overview/__tests__/fixtures/dashboardSummary'
import Dashboard from '../Dashboard.vue'

const api = vi.hoisted(() => ({ dashboardSummary: vi.fn(), summary: vi.fn(), dashboardTotal: vi.fn(), dashboardCharts: vi.fn() }))
const legacyDaily = vi.hoisted(() => vi.fn())
vi.mock('@/stores/auth', () => ({ useAuthStore: () => ({ canAccessAdmin: true, isAdmin: true, isAuditAdmin: false }) }))
vi.mock('@/api/overview', () => ({ overviewApi: api }))
vi.mock('@/api/dashboard', () => ({ dashboardApi: { getDailyStats: legacyDaily } }))
vi.mock('@/features/overview/dashboard/DashboardActivity.vue', async () => {
  const { defineComponent, h } = await import('vue')
  return {
    default: defineComponent({
      props: { data: Object, scopeHint: String, consecutiveActiveDays: Number, activeDays: Number },
      setup: props => () => h('section', {
        'data-dashboard-activity': '',
        title: props.scopeHint,
        'data-start-date': props.data?.start_date,
        'data-end-date': props.data?.end_date,
        'data-total-days': props.data?.total_days,
      }, [
        h('span', `${props.consecutiveActiveDays} / ${props.activeDays}`),
        ...((props.data?.days ?? []) as { date: string; requests: number }[]).map(day =>
          h('span', { 'data-activity-date': day.date }, String(day.requests))),
      ]),
    }),
  }
})
vi.mock('@/features/overview/dashboard/DashboardAnnouncements.vue', () => ({ default: { render: () => null } }))
vi.mock('@/components/charts/BarChart.vue', () => ({ default: { render: () => null } }))
vi.mock('@/components/charts/DoughnutChart.vue', () => ({ default: { render: () => null } }))
vi.mock('@/components/charts/LineChart.vue', () => ({ default: { render: () => null } }))

function dashboardCharts(range: OverviewRange = {
  from: '2026-09-19T00:00:00Z', to: '2026-09-19T04:00:00Z', timezone: 'UTC',
}): OverviewResponse<OverviewDashboardCharts> {
  const amount = { value: '0', currency: 'USD', basis: 'billable', status: 'known' }
  const summary: OverviewDashboardChartMetrics = {
    request_count: 0, successful_request_count: 0, failed_request_count: 0,
    cancelled_request_count: 0, in_flight_request_count: 0, unclassified_failure_count: null,
    input_tokens: null, output_tokens: null, total_tokens: 0, usage_active_users: null, slow_request_count: null,
    success_rate: { value: null, numerator: 0, denominator: 0 },
    latency_ms: { avg: null, p50: null, p95: null, p99: null, sample_count: 0 },
    rated_amount: amount, billable_amount: amount, quota_covered_amount: amount,
    wallet_consumed_amount: amount, wallet_debit_amount: amount,
  }
  return {
    meta: {
      schema_version: 1, metric_version: 'test', scope: { kind: 'admin' },
      range: { ...range, time_basis: 'request_started_at' },
      generated_at: range.to, data_through: range.to, read_revision: 'test',
      coverage: {
        status: 'complete', request_count: 0, usage_available_count: 0,
        pricing_available_count: 0, settled_count: 0,
      },
    },
    data: { summary, series: [], models: [], providers: [] },
  }
}

function deferred<T>() {
  let resolve!: (value: T) => void
  let reject!: (reason: Error) => void
  const promise = new Promise<T>((resolvePromise, rejectPromise) => { resolve = resolvePromise; reject = rejectPromise })
  return { promise, resolve, reject }
}
let app: App | undefined
function mount() {
  const root = document.createElement('div')
  app = createApp(Dashboard)
  app.mount(root)
  return root
}
async function settle() {
  for (let i = 0; i < 8; i += 1) { await Promise.resolve(); await nextTick() }
}
beforeEach(() => {
  vi.useFakeTimers()
  vi.resetAllMocks()
  api.dashboardSummary.mockResolvedValue(dashboardSummary())
  api.dashboardCharts.mockImplementation((range: OverviewRange) => Promise.resolve(dashboardCharts(range)))
})
afterEach(() => { app?.unmount(); app = undefined; vi.useRealTimers() })

describe('dashboard snapshot loading', () => {
  it('shows today and total from one compact snapshot independently of pending charts', async () => {
    api.dashboardCharts.mockReturnValue(new Promise(() => {}))
    const root = mount()
    await settle()
    expect(root.textContent).toContain('总请求 12,345')
    expect(root.textContent).toContain('$9.87')
    expect(root.textContent).toContain('1.52s')
    expect(root.querySelector('[aria-busy="true"]')).toBeNull()
    expect(api.dashboardSummary).toHaveBeenCalledWith(Intl.DateTimeFormat().resolvedOptions().timeZone, expect.any(AbortSignal))
    expect(api.summary).not.toHaveBeenCalled()
    expect(api.dashboardTotal).not.toHaveBeenCalled()
    expect(api.dashboardCharts).toHaveBeenCalledWith({
      from: expect.any(String), to: expect.any(String),
      timezone: Intl.DateTimeFormat().resolvedOptions().timeZone,
    }, expect.any(AbortSignal))
    expect(legacyDaily).not.toHaveBeenCalled()
    await vi.advanceTimersByTimeAsync(240_000)
    expect(api.dashboardSummary).toHaveBeenCalledTimes(1)
    expect(api.dashboardCharts).toHaveBeenCalledTimes(1)
  })

  it('labels restored history with its UTC activity dates independently of the dashboard timezone', async () => {
    const snapshot = dashboardSummary()
    snapshot.timezone = 'Asia/Shanghai'
    snapshot.activity_timezone = 'UTC'
    snapshot.stats_since = '2026-05-20T00:00:00Z'
    snapshot.activity_days = [{ date: '2026-05-20', requests: 1234 }, ...snapshot.activity_days]
    api.dashboardSummary.mockResolvedValue(snapshot)
    const root = mount()
    await settle()

    const activity = root.querySelector('[data-dashboard-activity]')
    expect(activity?.getAttribute('title')).toContain('展示近365天 · UTC')
    expect(activity?.getAttribute('title')).toContain('统计自 2026/5/20 08:00')
    expect(activity?.textContent).toContain('2 / 413')
    expect(activity?.querySelector('[data-activity-date="2026-05-20"]')?.textContent).toBe('1234')
  })

  it('uses the dashboard timezone for activity when the optional activity timezone is absent', async () => {
    const snapshot = dashboardSummary()
    snapshot.timezone = 'Asia/Shanghai'
    api.dashboardSummary.mockResolvedValue(snapshot)
    const root = mount()
    await settle()

    expect(root.querySelector('[data-dashboard-activity]')?.getAttribute('title'))
      .toContain('展示近365天 · Asia/Shanghai')
  })

  it.each([
    { activityTimezone: 'UTC', start: '2025-09-20', end: '2026-09-19' },
    { activityTimezone: 'Asia/Shanghai', start: '2025-09-21', end: '2026-09-20' },
    { activityTimezone: undefined, start: '2025-09-21', end: '2026-09-20' },
  ])('ends the 365-day calendar on $end for activity timezone $activityTimezone', async ({ activityTimezone, start, end }) => {
    const snapshot = dashboardSummary()
    snapshot.timezone = 'Asia/Shanghai'
    snapshot.activity_timezone = activityTimezone
    snapshot.generated_at = '2026-09-19T16:30:00Z'
    api.dashboardSummary.mockResolvedValue(snapshot)
    const root = mount()
    await settle()

    const activity = root.querySelector('[data-dashboard-activity]')
    const days = activity?.querySelectorAll('[data-activity-date]')
    expect(activity?.getAttribute('data-start-date')).toBe(start)
    expect(activity?.getAttribute('data-end-date')).toBe(end)
    expect(activity?.getAttribute('data-total-days')).toBe('365')
    expect(days).toHaveLength(365)
    expect(days?.[0]?.getAttribute('data-activity-date')).toBe(start)
    expect(days?.[364]?.getAttribute('data-activity-date')).toBe(end)
  })

  it('includes empty days at both ends and between requests without changing lifetime active counts', async () => {
    const snapshot = dashboardSummary()
    snapshot.activity_days = [
      { date: '2025-09-19', requests: 9999 },
      { date: '2026-09-16', requests: 50 },
      { date: '2026-09-18', requests: 10 },
      { date: '2026-09-20', requests: 9999 },
    ]
    api.dashboardSummary.mockResolvedValue(snapshot)
    const root = mount()
    await settle()

    const activity = root.querySelector('[data-dashboard-activity]')
    expect(activity?.querySelectorAll('[data-activity-date]')).toHaveLength(365)
    for (const date of ['2025-09-20', '2026-09-17', '2026-09-19']) {
      expect(activity?.querySelector(`[data-activity-date="${date}"]`)?.textContent).toBe('0')
    }
    expect(activity?.querySelector('[data-activity-date="2026-09-16"]')?.textContent).toBe('50')
    expect(activity?.querySelector('[data-activity-date="2026-09-18"]')?.textContent).toBe('10')
    expect(activity?.querySelector('[data-activity-date="2025-09-19"]')).toBeNull()
    expect(activity?.querySelector('[data-activity-date="2026-09-20"]')).toBeNull()
    expect(activity?.textContent).toContain('2 / 413')
  })

  it('shows all 365 zero-request days when no usage history exists', async () => {
    const snapshot = dashboardSummary()
    snapshot.activity_days = []
    snapshot.active_days = 0
    snapshot.consecutive_active_days = 0
    api.dashboardSummary.mockResolvedValue(snapshot)
    const root = mount()
    await settle()

    const activity = root.querySelector('[data-dashboard-activity]')
    const days = activity?.querySelectorAll('[data-activity-date]')
    expect(days).toHaveLength(365)
    expect(Array.from(days ?? []).every(day => day.textContent === '0')).toBe(true)
    expect(activity?.getAttribute('data-start-date')).toBe('2025-09-20')
    expect(activity?.getAttribute('data-end-date')).toBe('2026-09-19')
    expect(activity?.textContent).toContain('0 / 0')
  })

  it('does not loop requests with the real time picker when summary fails', async () => {
    const snapshot = deferred<OverviewDashboardSummary>()
    api.dashboardSummary.mockReturnValue(snapshot.promise)
    const root = mount()
    await settle()
    expect(root.querySelector('[aria-busy="true"]')).not.toBeNull()
    snapshot.reject(new Error('timeout'))
    await settle()
    expect(root.querySelector('[aria-busy="true"]')).toBeNull()
    expect(root.querySelector('[role="alert"]')).toBeNull()
    expect(root.textContent).not.toContain('加载失败')
    expect(root.textContent).not.toContain('重试')
    expect(root.querySelector('[data-request-metric="stream"]')?.textContent).toContain('—')
    await vi.advanceTimersByTimeAsync(61_000)
    expect(api.dashboardSummary).toHaveBeenCalledTimes(1)
    expect(api.dashboardCharts.mock.calls.length).toBeLessThanOrEqual(2)
    expect(legacyDaily).not.toHaveBeenCalled()
  })

  it('aborts the chart request when unmounted and ignores late success', async () => {
    const charts = deferred<OverviewResponse<OverviewDashboardCharts>>()
    api.dashboardCharts.mockReturnValue(charts.promise)
    const root = mount()
    await settle()
    const [range, signal] = api.dashboardCharts.mock.calls[0] as [OverviewRange, AbortSignal]
    expect(signal.aborted).toBe(false)
    expect(root.textContent).toContain('总请求 12,345')
    app?.unmount()
    app = undefined
    expect(signal.aborted).toBe(true)
    const response = dashboardCharts(range)
    response.data.series = [{ ...response.data.summary, bucket_start: range.from, request_count: 987654 }]
    charts.resolve(response)
    await settle()
    expect(root.textContent).toBe('')
    await vi.advanceTimersByTimeAsync(240_000)
    expect(api.dashboardCharts).toHaveBeenCalledTimes(1)
    expect(legacyDaily).not.toHaveBeenCalled()
  })

  it('aborts the compact snapshot request when unmounted and ignores late success', async () => {
    const snapshot = deferred<OverviewDashboardSummary>()
    api.dashboardSummary.mockReturnValue(snapshot.promise)
    const root = mount()
    await settle()
    const signal = api.dashboardSummary.mock.calls[0]![1] as AbortSignal
    app?.unmount()
    app = undefined
    expect(signal.aborted).toBe(true)
    snapshot.resolve(dashboardSummary())
    await settle()
    expect(root.textContent).toBe('')
    await vi.advanceTimersByTimeAsync(240_000)
    expect(api.dashboardSummary).toHaveBeenCalledTimes(1)
  })
})
