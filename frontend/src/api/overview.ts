import apiClient from './client'
import type { GatewayMetricsSummary, AdminMonitoringResilienceStatus } from './monitoring'
import type { ProviderPerformanceResponse } from './admin'

export interface OverviewRange {
  from: string
  to: string
  timezone: string
}

export interface OverviewQuery extends OverviewRange {
  user_id?: string
  api_key_id?: string
  request_type?: string
  credential_owner_id?: string
  attribution_kind?: string
  model?: string
  provider_id?: string
  api_format?: string
  endpoint_kind?: string
  is_stream?: boolean
  has_format_conversion?: boolean
  slow_threshold_ms?: number
  status?: string
  search?: string
  account_status?: string
  usage_status?: string
  sort?: string
  order?: 'asc' | 'desc'
  group_by?: string
  amount_basis?: string
  granularity?: 'hour' | 'day'
  limit?: number
  offset?: number
  payment_limit?: number
  payment_offset?: number
}

export interface OverviewAmount {
  value: string | null
  currency: string
  basis: string
  status: 'known' | 'known_subtotal' | 'unknown' | 'estimated' | string
}

export interface OverviewMetrics {
  request_count: number
  successful_request_count: number
  failed_request_count: number
  cancelled_request_count: number
  in_flight_request_count: number
  unclassified_failure_count: number
  input_tokens: number | null
  output_tokens: number | null
  total_tokens: number | null
  usage_source?: 'reported' | 'estimated' | 'mixed' | 'unknown'
  usage_source_counts?: { reported: number; estimated: number; mixed: number; unknown: number }
  requests_per_second?: number
  requests_per_minute?: number
  tokens_per_minute?: number | null
  window_seconds?: number
  usage_active_users?: number
  enabled_users?: number
  success_rate: { value: number | null; numerator: number; denominator: number }
  latency_ms: { avg: number | null; p50: number | null; p95: number | null; p99: number | null; sample_count: number }
  rated_amount: OverviewAmount
  billable_amount: OverviewAmount
  quota_covered_amount: OverviewAmount
  wallet_consumed_amount: OverviewAmount
  wallet_debit_amount: OverviewAmount
}

export interface OverviewMeta {
  schema_version: number
  metric_version: string
  scope: { kind: string; user_id?: string }
  range: OverviewRange & { time_basis: string }
  generated_at: string
  data_through: string | null
  read_revision: string
  available_metrics?: string[]
  projection?: {
    projection_from: string | null
    projection_through: string | null
    dirty_bucket_count: number
    missing_bucket_count: number
    read_enabled: boolean
  }
  coverage: {
    status: string
    request_count: number
    usage_available_count: number
    pricing_available_count: number
    settled_count: number
    attribution_available_count?: number
    unrecoverable_bucket_count?: number
  }
}

export interface OverviewResponse<T> { meta: OverviewMeta; data: T }
export type OverviewDashboardTotals = Pick<OverviewMetrics, 'request_count' | 'total_tokens' | 'billable_amount' | 'enabled_users'>
export interface OverviewDashboard {
  today: OverviewResponse<OverviewMetrics>
  total: OverviewResponse<OverviewDashboardTotals>
  history_complete: boolean | null
}
export type OverviewDashboardTotal = { status: 'pending' } | {
  status: 'ready'
  total: OverviewResponse<OverviewDashboardTotals>
  history_complete: boolean | null
  stale: boolean
}
export interface OverviewDashboardSummary {
  stats_since: string
  generated_at: string
  timezone: string
  activity_timezone?: string
  today_from: string
  window_seconds: number
  today: {
    request_count: number
    input_tokens: number | null
    output_tokens: number | null
    total_tokens: number | null
    billable_amount: OverviewAmount
    active_users: number
    cache_read_tokens: number | null
    cache_creation_tokens: number | null
    cache_input_tokens: number | null
    avg_first_byte_ms: number | null
    avg_response_ms: number | null
    stream_requests: number
    standard_requests: number
  }
  total: {
    request_count: number
    total_tokens: number | null
    billable_amount: OverviewAmount
    cache_read_tokens: number | null
    cache_input_tokens: number | null
  }
  users: { total: number; created_today: number; deleted_today: number }
  consecutive_active_days: number
  active_days: number
  activity_days: { date: string; requests: number }[]
  concurrency: {
    avg: number | null
    peak: number | null
    observed_from: string | null
    observed_through: string | null
    scope: 'node'
    coverage: 'partial' | 'complete' | 'unavailable'
  }
}
export interface OverviewPage<T> { items: T[]; total: number; limit: number; offset: number }
export interface OverviewSeriesPoint extends OverviewMetrics { bucket_start: string; unique_providers?: number | null }
export interface OverviewBreakdown extends OverviewMetrics { id: string | null; label: string | null }
export interface OverviewDashboardChartMetrics extends Omit<OverviewMetrics, 'usage_active_users' | 'unclassified_failure_count'> {
  usage_active_users: number | null
  slow_request_count: number | null
  unclassified_failure_count: number | null
}
export interface OverviewDashboardCharts {
  summary: OverviewDashboardChartMetrics
  series: (OverviewDashboardChartMetrics & { bucket_start: string; unique_providers?: number | null })[]
  models: (OverviewDashboardChartMetrics & { id: string | null; label: string | null; bucket_start: string })[]
  providers: (OverviewDashboardChartMetrics & { id: string | null; label: string | null })[]
}
export interface OverviewEmployee extends OverviewMetrics {
  user_id: string
  username: string
  email: string | null
  is_active: boolean
  last_used_at: string | null
  active_days: number
  finance?: OverviewUserFinance | null
}
export interface OverviewUserFinance {
  wallet_balance: OverviewAmount
  recharge_balance: OverviewAmount
  gift_balance: OverviewAmount
  recharge_amount: OverviewAmount
  recharge_count: number
  plan_purchase_amount: OverviewAmount
  plan_purchase_count: number
  gift_credit_amount: OverviewAmount
  gift_credit_count: number
  balance_time_basis: 'current'
  payment_time_basis: 'credited_at'
}
export interface OverviewUserPayment {
  id: string
  order_no: string
  kind: 'wallet_recharge' | 'plan_purchase' | 'gift_credit'
  amount: OverviewAmount
  payment_method: string
  credited_at: string
}
export interface OverviewUsers extends OverviewPage<OverviewEmployee> {
  summary?: OverviewMetrics & { user_count: number; active_user_count: number }
  finance_summary?: OverviewUserFinance | null
}
export interface OverviewEmployeeDetail {
  user: { id: string; username: string; email: string | null; is_active: boolean }
  summary: OverviewMetrics
  finance?: OverviewUserFinance | null
  payments?: OverviewPage<OverviewUserPayment> | null
}
export interface OverviewConsumption {
  id: string
  request_id: string
  started_at: string
  user_id: string | null
  model: string | null
  provider: string | null
  status: string
  settlement_status: string
  attribution_kind: string
  rated_amount: OverviewAmount
  billable_amount: OverviewAmount
  quota_covered_amount: OverviewAmount
  wallet_consumed_amount: OverviewAmount
  wallet_debit_amount: OverviewAmount
}
export interface OverviewCosts {
  summary: OverviewMetrics
  timeseries?: OverviewSeriesPoint[]
  supplier_estimated_cost: OverviewAmount
  supplier_verified_cost: OverviewAmount
  cache: { read_tokens: number | null; creation_tokens: number | null; read_cost: OverviewAmount; creation_cost: OverviewAmount; estimated_full_cost: OverviewAmount; estimated_savings: OverviewAmount; pricing_available_count?: number; request_count?: number }
  forecast: { amount: OverviewAmount; method: string; status: string; sample_days: number; period_end: string | null }
}
export interface OverviewExecutionActivity {
  observed_at: string
  observed_from: string
  window_seconds: number
  observed_window_seconds: number
  scope: { kind: 'node' }
  coverage: 'complete' | 'partial'
  providers: { provider_id: string; provider: string | null; requests_per_minute: number; current_concurrency: number }[]
  models: { model: string | null; requests_per_minute: number; current_concurrency: number }[]
}
export interface OverviewLive {
  observed_at: string | null
  window_seconds: number | null
  node_id: string | null
  scope: { kind: string; node_ids?: string[] }
  metrics?: GatewayMetricsSummary | null
  metrics_text?: string | null
  resilience: AdminMonitoringResilienceStatus | null
  unavailable_sections: string[]
  recent_activity?: OverviewResponse<OverviewMetrics>
  execution_activity?: OverviewExecutionActivity | null
}
export interface OverviewModelPerformance {
  model: string | null
  request_count: number
  success_count: number
  error_count: number
  success_rate: number | null
  avg_first_byte_time_ms: number | null
  avg_output_tps: number | null
  avg_response_time_ms: number | null
}
export interface OverviewPerformance {
  summary: OverviewMetrics
  timeseries: OverviewSeriesPoint[]
  providers: ProviderPerformanceResponse | null
  models?: OverviewModelPerformance[]
  errors: { reason: string; count: number }[]
}

async function get<T>(path: string, params?: OverviewQuery, signal?: AbortSignal): Promise<OverviewResponse<T>> {
  return (await apiClient.get<OverviewResponse<T>>(`/api/admin/overview/${path}`, { params, signal })).data
}

export const overviewApi = {
  async dashboardSummary(timezone: string, signal?: AbortSignal): Promise<OverviewDashboardSummary> {
    return (await apiClient.get<OverviewDashboardSummary>('/api/admin/overview/dashboard/summary', { params: { timezone }, signal })).data
  },
  async dashboard(timezone: string, signal?: AbortSignal): Promise<OverviewDashboard> {
    return (await apiClient.get<OverviewDashboard>('/api/admin/overview/dashboard', { params: { timezone }, signal })).data
  },
  async dashboardTotal(timezone: string, signal?: AbortSignal): Promise<OverviewDashboardTotal> {
    return (await apiClient.get<OverviewDashboardTotal>('/api/admin/overview/dashboard/total', { params: { timezone }, signal })).data
  },
  async dashboardCharts(range: OverviewRange, signal?: AbortSignal): Promise<OverviewResponse<OverviewDashboardCharts>> {
    const { from, to, timezone } = range
    return (await apiClient.get<OverviewResponse<OverviewDashboardCharts>>('/api/admin/overview/dashboard/charts', { params: { from, to, timezone, granularity: 'day' }, signal })).data
  },
  summary: (query: OverviewQuery, signal?: AbortSignal) => get<OverviewMetrics>('summary', query, signal),
  timeseries: (query: OverviewQuery, signal?: AbortSignal) => get<{ items: OverviewSeriesPoint[]; granularity: string }>('timeseries', query, signal),
  breakdown: (query: OverviewQuery, signal?: AbortSignal) => get<OverviewPage<OverviewBreakdown>>('breakdown', query, signal),
  users: (query: OverviewQuery, signal?: AbortSignal) => get<OverviewUsers>('users', query, signal),
  user: (id: string, query: OverviewQuery, signal?: AbortSignal) => get<OverviewEmployeeDetail>(`users/${encodeURIComponent(id)}`, query, signal),
  consumption: (query: OverviewQuery, signal?: AbortSignal) => get<OverviewPage<OverviewConsumption>>('consumption', query, signal),
  costs: (query: OverviewQuery, signal?: AbortSignal) => get<OverviewCosts>('costs', query, signal),
  performance: (query: OverviewQuery, signal?: AbortSignal) => get<OverviewPerformance>('operations/performance', query, signal),
  live: (signal?: AbortSignal) => get<OverviewLive>('operations/live', undefined, signal),
  resources: (signal?: AbortSignal) => get<OverviewLive>('operations/resources', undefined, signal),
  async exportCsv(path: 'users' | 'consumption' | 'breakdown', query: OverviewQuery, signal?: AbortSignal): Promise<Blob> {
    const { limit: _limit, offset: _offset, ...filters } = query
    return (await apiClient.get<Blob>(`/api/admin/overview/${path}`, {
      params: { ...filters, format: 'csv' }, responseType: 'blob', signal,
    })).data
  },
}
