import apiClient from './client'
import { cachedRequest, dedupedRequest, buildCacheKey } from '@/utils/cache'
import type { ActivityHeatmap } from '@/types/activity'
import type { ImageProgress } from './requestTrace'

const ACTIVITY_HEATMAP_CACHE_TTL_MS = 30 * 60 * 1000
const USAGE_ANALYTICS_CACHE_TTL_MS = 30 * 1000
const USAGE_ANALYTICS_REQUEST_TIMEOUT_MS = 120 * 1000

export interface UsageRecord {
  id: string // UUID
  user_id: string // UUID
  username?: string
  provider_id?: string // UUID
  provider_name?: string
  model: string
  request_type?: string | null
  requested_reasoning_effort?: string | null
  reasoning_effort?: string | null
  service_tier?: string | null
  actual_service_tier?: string | null
  input_tokens: number
  effective_input_tokens?: number
  output_tokens: number
  cache_creation_input_tokens?: number
  cache_creation_ephemeral_5m_input_tokens?: number
  cache_creation_ephemeral_1h_input_tokens?: number
  cache_read_input_tokens?: number
  total_tokens: number
  cost?: number
  response_time?: number
  created_at: string
  updated_at?: string | null
  response_time_updated_at?: string | null
  has_fallback?: boolean // 🆕 是否发生了 fallback
  client_family?: string | null
  client_ip?: string | null
  user_agent?: string | null
  request_path?: string | null
  request_path_and_query?: string | null
}

export interface UsageStats {
  total_requests: number
  total_tokens: number
  total_cost: number
  total_actual_cost?: number
  avg_response_time: number
  error_count?: number
  error_rate?: number
  today?: {
    requests: number
    tokens: number
    cost: number
  }
  activity_heatmap?: ActivityHeatmap | null
}

export interface UsageByModel {
  model: string
  request_count: number
  total_tokens: number
  effective_input_tokens?: number
  total_input_context?: number
  output_tokens?: number
  cache_creation_tokens?: number
  total_cost: number
  avg_response_time?: number
  cache_read_tokens?: number
  cache_hit_rate?: number
}

export interface UsageByUser {
  user_id: string // UUID
  email: string
  username: string
  request_count: number
  total_tokens: number
  total_cost: number
}

export interface UsageByProvider {
  provider_id?: string | null
  provider_key?: string
  provider_identity_source?: 'provider_id' | 'legacy_name'
  provider: string
  request_count: number
  total_tokens: number
  effective_input_tokens?: number
  total_input_context?: number
  output_tokens?: number
  cache_creation_tokens?: number
  total_cost: number
  actual_cost: number
  avg_response_time_ms: number
  success_rate: number
  error_count: number
  cache_read_tokens?: number
  cache_hit_rate?: number
}

export interface UsageByApiFormat {
  api_format: string
  request_count: number
  total_tokens: number
  effective_input_tokens?: number
  total_input_context?: number
  output_tokens?: number
  cache_creation_tokens?: number
  total_cost: number
  actual_cost: number
  avg_response_time_ms: number
  cache_read_tokens?: number
  cache_hit_rate?: number
}

export interface UsageFilters {
  user_id?: string // UUID
  provider_id?: string // UUID
  model?: string
  search?: string
  start_date?: string
  end_date?: string
  preset?: string
  granularity?: 'hour' | 'day' | 'week' | 'month'
  timezone?: string
  tz_offset_minutes?: number
  client_family?: string
  page?: number
  page_size?: number
}

export interface UsageRequestOptions {
  skipCache?: boolean
}

type UsageListResponse = {
  records?: unknown
  pagination?: {
    total?: unknown
    limit?: unknown
    offset?: unknown
  }
  total?: unknown
  limit?: unknown
  offset?: unknown
}

function assertPositiveInteger(value: number, field: string): number {
  if (!Number.isInteger(value) || value < 1) {
    throw new Error(`${field} must be a positive integer`)
  }
  return value
}

function assertNonNegativeInteger(value: number, field: string): number {
  if (!Number.isInteger(value) || value < 0) {
    throw new Error(`${field} must be a non-negative integer`)
  }
  return value
}

function assertNumber(value: unknown, field: string): number {
  if (typeof value !== 'number' || Number.isNaN(value)) {
    throw new Error(`Usage response is missing numeric ${field}`)
  }
  return value
}

function assertUsageRecords(value: unknown): UsageRecord[] {
  if (!Array.isArray(value)) {
    throw new Error('Usage response is missing records array')
  }
  return value as UsageRecord[]
}

function compactParams(params: Record<string, unknown>): Record<string, unknown> {
  return Object.fromEntries(
    Object.entries(params).filter(([, value]) => value !== undefined && value !== null && value !== '')
  )
}

function offsetPaginationFromPage(filters?: Pick<UsageFilters, 'page' | 'page_size'>): {
  page: number
  pageSize: number | undefined
  offset: number | undefined
} {
  const page = assertPositiveInteger(filters?.page ?? 1, 'page')
  if (filters?.page_size === undefined) {
    return { page, pageSize: undefined, offset: undefined }
  }

  const pageSize = assertPositiveInteger(filters.page_size, 'page_size')
  return {
    page,
    pageSize,
    offset: assertNonNegativeInteger((page - 1) * pageSize, 'offset'),
  }
}

function normalizeUsageRecordPage(
  payload: UsageListResponse,
  requested: { page: number; pageSize?: number; offset?: number }
): {
  records: UsageRecord[]
  total: number
  page: number
  page_size: number
} {
  const records = assertUsageRecords(payload.records)
  const pagination = payload.pagination
  const total = assertNumber(pagination?.total ?? payload.total, 'pagination.total')
  const limit = assertPositiveInteger(
    assertNumber(pagination?.limit ?? payload.limit, 'pagination.limit'),
    'pagination.limit'
  )
  const offset = assertNonNegativeInteger(
    assertNumber(pagination?.offset ?? payload.offset, 'pagination.offset'),
    'pagination.offset'
  )
  const resolvedPage = requested.pageSize !== undefined
    ? requested.page
    : Math.floor(offset / limit) + 1

  return {
    records,
    total,
    page: resolvedPage,
    page_size: limit,
  }
}

function buildCurrentUserUsageParams(filters?: UsageFilters): {
  params: Record<string, unknown>
  pagination: { page: number; pageSize?: number; offset?: number }
} {
  if (filters?.user_id || filters?.provider_id || filters?.model || filters?.granularity) {
    throw new Error('getUsageRecords only supports current-user usage filters; use admin usage APIs for user/model/provider filters')
  }

  const pagination = offsetPaginationFromPage(filters)
  return {
    pagination,
    params: compactParams({
      start_date: filters?.start_date,
      end_date: filters?.end_date,
      preset: filters?.preset,
      timezone: filters?.timezone,
      tz_offset_minutes: filters?.tz_offset_minutes,
      search: filters?.search,
      limit: pagination.pageSize,
      offset: pagination.offset,
    }),
  }
}

function buildAdminUsageRecordParams(userId: string, filters?: UsageFilters): {
  params: Record<string, unknown>
} {
  if (!userId.trim()) {
    throw new Error('getUserUsage requires a non-empty user id')
  }
  if (filters?.provider_id || filters?.granularity) {
    throw new Error('getUserUsage does not support provider_id or granularity filters')
  }

  const pagination = offsetPaginationFromPage(filters)
  return {
    params: compactParams({
      user_id: userId,
      start_date: filters?.start_date,
      end_date: filters?.end_date,
      preset: filters?.preset,
      timezone: filters?.timezone,
      tz_offset_minutes: filters?.tz_offset_minutes,
      search: filters?.search,
      model: filters?.model,
      limit: pagination.pageSize,
      offset: pagination.offset,
    }),
  }
}

function buildAdminUsageStatsParams(userId: string, filters?: UsageFilters): Record<string, unknown> {
  if (!userId.trim()) {
    throw new Error('getUserUsage requires a non-empty user id')
  }
  if (filters?.provider_id || filters?.granularity) {
    throw new Error('getUserUsage stats does not support provider_id or granularity filters')
  }

  return compactParams({
    user_id: userId,
    start_date: filters?.start_date,
    end_date: filters?.end_date,
    preset: filters?.preset,
    timezone: filters?.timezone,
    tz_offset_minutes: filters?.tz_offset_minutes,
    model: filters?.model,
  })
}

function normalizeActivityHeatmapResponse(payload: unknown): ActivityHeatmap {
  const today = new Date()
  const endDate = today.toISOString().slice(0, 10)
  const start = new Date(today)
  start.setUTCDate(start.getUTCDate() - 364)
  const startDate = start.toISOString().slice(0, 10)

  if (payload && typeof payload === 'object' && !Array.isArray(payload)) {
    const candidate = payload as Partial<ActivityHeatmap>
    if (Array.isArray(candidate.days)) {
      return {
        start_date: typeof candidate.start_date === 'string' ? candidate.start_date : startDate,
        end_date: typeof candidate.end_date === 'string' ? candidate.end_date : endDate,
        total_days: typeof candidate.total_days === 'number' ? candidate.total_days : candidate.days.length,
        max_requests: typeof candidate.max_requests === 'number' ? candidate.max_requests : 0,
        days: candidate.days,
      }
    }
  }

  const grouped = new Map<string, { requests: number; total_tokens: number; total_cost: number; actual_total_cost?: number }>()
  if (Array.isArray(payload)) {
    for (const item of payload) {
      if (!item || typeof item !== 'object') continue
      const raw = item as Record<string, unknown>
      const date = typeof raw.date === 'string' ? raw.date : ''
      if (!date) continue
      grouped.set(date, {
        requests: typeof raw.requests === 'number'
          ? raw.requests
          : typeof raw.request_count === 'number'
            ? raw.request_count
            : 0,
        total_tokens: typeof raw.total_tokens === 'number' ? raw.total_tokens : 0,
        total_cost: typeof raw.total_cost === 'number' ? raw.total_cost : 0,
        actual_total_cost: typeof raw.actual_total_cost === 'number' ? raw.actual_total_cost : undefined,
      })
    }
  }

  const days: ActivityHeatmap['days'] = []
  let maxRequests = 0
  const cursor = new Date(start)
  while (cursor <= today) {
    const date = cursor.toISOString().slice(0, 10)
    const existing = grouped.get(date)
    const requests = existing?.requests ?? 0
    maxRequests = Math.max(maxRequests, requests)
    days.push({
      date,
      requests,
      total_tokens: existing?.total_tokens ?? 0,
      total_cost: existing?.total_cost ?? 0,
      actual_total_cost: existing?.actual_total_cost,
    })
    cursor.setUTCDate(cursor.getUTCDate() + 1)
  }

  return {
    start_date: startDate,
    end_date: endDate,
    total_days: days.length,
    max_requests: maxRequests,
    days,
  }
}

export const usageApi = {
  async getUsageRecords(filters?: UsageFilters): Promise<{
    records: UsageRecord[]
    total: number
    page: number
    page_size: number
  }> {
    const { params, pagination } = buildCurrentUserUsageParams(filters)
    const response = await apiClient.get<UsageListResponse>('/api/users/me/usage', { params })
    return normalizeUsageRecordPage(response.data, pagination)
  },

  async getUsageStats(filters?: UsageFilters, options?: UsageRequestOptions): Promise<UsageStats> {
    // 为统计数据添加30秒缓存
    const cacheKey = `usage-stats-${JSON.stringify(filters || {})}${options?.skipCache ? ':fresh' : ''}`
    return cachedRequest(
      cacheKey,
      async () => {
        const response = await apiClient.get<UsageStats>('/api/admin/usage/stats', {
          params: filters,
          timeout: USAGE_ANALYTICS_REQUEST_TIMEOUT_MS,
        })
        return response.data
      },
      options?.skipCache ? 0 : USAGE_ANALYTICS_CACHE_TTL_MS
    )
  },

  /**
   * Get usage aggregation by dimension (RESTful API)
   * @param groupBy Aggregation dimension: 'model', 'user', 'provider', or 'api_format'
   * @param filters Optional filters
   */
  async getUsageAggregation<T = UsageByModel[] | UsageByUser[] | UsageByProvider[] | UsageByApiFormat[]>(
    groupBy: 'model' | 'user' | 'provider' | 'api_format',
    filters?: UsageFilters & { limit?: number },
    options?: UsageRequestOptions
  ): Promise<T> {
    const cacheKey = `usage-aggregation-${groupBy}-${JSON.stringify(filters || {})}${options?.skipCache ? ':fresh' : ''}`
    return cachedRequest(
      cacheKey,
      async () => {
        const response = await apiClient.get<T>('/api/admin/usage/aggregation/stats', {
          params: { group_by: groupBy, ...filters },
          timeout: USAGE_ANALYTICS_REQUEST_TIMEOUT_MS,
        })
        return response.data
      },
      options?.skipCache ? 0 : USAGE_ANALYTICS_CACHE_TTL_MS
    )
  },

  // Shorthand methods using getUsageAggregation
  async getUsageByModel(
    filters?: UsageFilters & { limit?: number },
    options?: UsageRequestOptions
  ): Promise<UsageByModel[]> {
    return this.getUsageAggregation<UsageByModel[]>('model', filters, options)
  },

  async getUsageByUser(
    filters?: UsageFilters & { limit?: number },
    options?: UsageRequestOptions
  ): Promise<UsageByUser[]> {
    return this.getUsageAggregation<UsageByUser[]>('user', filters, options)
  },

  async getUsageByProvider(
    filters?: UsageFilters & { limit?: number },
    options?: UsageRequestOptions
  ): Promise<UsageByProvider[]> {
    return this.getUsageAggregation<UsageByProvider[]>('provider', filters, options)
  },

  async getUsageByApiFormat(
    filters?: UsageFilters & { limit?: number },
    options?: UsageRequestOptions
  ): Promise<UsageByApiFormat[]> {
    return this.getUsageAggregation<UsageByApiFormat[]>('api_format', filters, options)
  },

  async getUserUsage(userId: string, filters?: UsageFilters): Promise<{
    records: UsageRecord[]
    stats: UsageStats
  }> {
    const statsParams = buildAdminUsageStatsParams(userId, filters)
    const { params: recordParams } = buildAdminUsageRecordParams(userId, filters)
    const [statsResponse, recordsResponse] = await Promise.all([
      apiClient.get<UsageStats>('/api/admin/usage/stats', { params: statsParams }),
      apiClient.get<UsageListResponse>('/api/admin/usage/records', { params: recordParams }),
    ])

    return {
      records: assertUsageRecords(recordsResponse.data.records),
      stats: statsResponse.data,
    }
  },

  async getAllUsageRecords(params?: {
    start_date?: string
    end_date?: string
    preset?: string
    granularity?: 'hour' | 'day' | 'week' | 'month'
    timezone?: string
    tz_offset_minutes?: number
    search?: string  // 通用搜索：用户名、密钥名、模型名、提供商名
    user_id?: string // UUID
    username?: string
    model?: string
    provider?: string
    api_format?: string  // API 格式筛选（如 openai:chat, claude:messages）
    status?: string // 'stream' | 'standard' | 'error'
    client_family?: string
    hide_unknown?: boolean
    include_total?: boolean
    total_only?: boolean
    limit?: number
    offset?: number
  }): Promise<{
    records: Array<Record<string, unknown>>
    total: number
    limit: number
    offset: number
    total_is_estimated?: boolean
  }> {
    const key = buildCacheKey('usage:records', params as Record<string, unknown> | undefined)
    return dedupedRequest(key, async () => {
      const response = await apiClient.get('/api/admin/usage/records', { params })
      return response.data
    })
  },

  async getAllUsageRecordTotal(params?: {
    start_date?: string
    end_date?: string
    preset?: string
    timezone?: string
    tz_offset_minutes?: number
    search?: string
    user_id?: string
    username?: string
    model?: string
    provider?: string
    api_format?: string
    status?: string
    client_family?: string
    hide_unknown?: boolean
  }): Promise<number> {
    const requestParams = compactParams({
      ...params,
      include_total: true,
      total_only: true,
      limit: 1,
      offset: 0,
    })
    const key = buildCacheKey('usage:records:total', requestParams)
    return dedupedRequest(key, async () => {
      const response = await apiClient.get<UsageListResponse>('/api/admin/usage/records', {
        params: requestParams,
      })
      return assertNumber(response.data.total, 'total')
    })
  },

  /**
   * 获取活跃请求的状态（轻量级接口，用于轮询更新）
   * @param ids 可选，逗号分隔的请求 ID 列表
   */
  async getActiveRequests(
    ids?: string[],
    timeRange?: Pick<UsageFilters, 'start_date' | 'end_date' | 'preset' | 'timezone' | 'tz_offset_minutes'>
  ): Promise<{
    requests: Array<{
      id: string
      status: 'pending' | 'streaming' | 'completed' | 'failed' | 'cancelled'
      input_tokens: number
      effective_input_tokens?: number | null
      output_tokens: number
      cache_creation_input_tokens?: number | null
      cache_creation_ephemeral_5m_input_tokens?: number | null
      cache_creation_ephemeral_1h_input_tokens?: number | null
      cache_read_input_tokens?: number | null
      cost: number
      actual_cost?: number | null
      rate_multiplier?: number | null
      response_time_ms: number | null
      first_byte_time_ms: number | null
      updated_at?: string | null
      response_time_updated_at?: string | null
      status_code?: number | null
      error_message?: string | null
      provider?: string | null
      api_key_name?: string | null
      provider_key_name?: string | null
      api_format?: string | null
      endpoint_api_format?: string | null
      is_stream?: boolean | null
      upstream_is_stream?: boolean | null
      client_requested_stream?: boolean | null
      client_is_stream?: boolean | null
      has_format_conversion?: boolean | null
      has_fallback?: boolean | null
      target_model?: string | null
      request_type?: string | null
      requested_reasoning_effort?: string | null
      reasoning_effort?: string | null
      service_tier?: string | null
      actual_service_tier?: string | null
      image_progress?: ImageProgress | null
    }>
  }> {
    const params: Record<string, string | number> = {}
    if (ids?.length) {
      params.ids = ids.join(',')
    }
    if (timeRange?.start_date) {
      params.start_date = timeRange.start_date
    }
    if (timeRange?.end_date) {
      params.end_date = timeRange.end_date
    }
    if (timeRange?.preset) {
      params.preset = timeRange.preset
    }
    if (timeRange?.timezone) {
      params.timezone = timeRange.timezone
    }
    if (typeof timeRange?.tz_offset_minutes === 'number') {
      params.tz_offset_minutes = timeRange.tz_offset_minutes
    }
    const response = await apiClient.get('/api/admin/usage/active', { params })
    return response.data
  },

  /**
   * 获取活跃度热力图数据（管理员）
   * 历史热力图变化很慢，前端做长缓存，避免自动刷新链路重复请求。
   */
  async getActivityHeatmap(): Promise<ActivityHeatmap> {
    return cachedRequest(
      'admin-usage-activity-heatmap',
      async () => {
        const response = await apiClient.get<ActivityHeatmap | unknown[]>('/api/admin/usage/heatmap')
        return normalizeActivityHeatmapResponse(response.data)
      },
      ACTIVITY_HEATMAP_CACHE_TTL_MS
    )
  }
}
