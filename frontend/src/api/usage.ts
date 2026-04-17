import apiClient from './client'
import { cachedRequest, dedupedRequest, buildCacheKey } from '@/utils/cache'
import type { ActivityHeatmap } from '@/types/activity'

export interface UsageRecord {
  id: string // UUID
  user_id: string // UUID
  username?: string
  provider_id?: string // UUID
  provider_name?: string
  model: string
  input_tokens: number
  effective_input_tokens?: number
  output_tokens: number
  cache_creation_input_tokens?: number
  cache_read_input_tokens?: number
  total_tokens: number
  cost?: number
  response_time?: number
  created_at: string
  has_fallback?: boolean // 🆕 是否发生了 fallback
}

export interface UsageStats {
  total_requests: number
  total_tokens: number
  total_cost: number
  total_actual_cost?: number
  avg_response_time: number
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
  provider_id: string
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
  start_date?: string
  end_date?: string
  preset?: string
  granularity?: 'hour' | 'day' | 'week' | 'month'
  timezone?: string
  tz_offset_minutes?: number
  page?: number
  page_size?: number
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
    const response = await apiClient.get('/api/usage', { params: filters })
    return response.data
  },

  async getUsageStats(filters?: UsageFilters): Promise<UsageStats> {
    // 为统计数据添加30秒缓存
    const cacheKey = `usage-stats-${JSON.stringify(filters || {})}`
    return cachedRequest(
      cacheKey,
      async () => {
        const response = await apiClient.get<UsageStats>('/api/admin/usage/stats', { params: filters })
        return response.data
      },
      30000 // 30秒缓存
    )
  },

  /**
   * Get usage aggregation by dimension (RESTful API)
   * @param groupBy Aggregation dimension: 'model', 'user', 'provider', or 'api_format'
   * @param filters Optional filters
   */
  async getUsageAggregation<T = UsageByModel[] | UsageByUser[] | UsageByProvider[] | UsageByApiFormat[]>(
    groupBy: 'model' | 'user' | 'provider' | 'api_format',
    filters?: UsageFilters & { limit?: number }
  ): Promise<T> {
    const cacheKey = `usage-aggregation-${groupBy}-${JSON.stringify(filters || {})}`
    return cachedRequest(
      cacheKey,
      async () => {
        const response = await apiClient.get<T>('/api/admin/usage/aggregation/stats', {
          params: { group_by: groupBy, ...filters }
        })
        return response.data
      },
      30000 // 30秒缓存
    )
  },

  // Shorthand methods using getUsageAggregation
  async getUsageByModel(filters?: UsageFilters & { limit?: number }): Promise<UsageByModel[]> {
    return this.getUsageAggregation<UsageByModel[]>('model', filters)
  },

  async getUsageByUser(filters?: UsageFilters & { limit?: number }): Promise<UsageByUser[]> {
    return this.getUsageAggregation<UsageByUser[]>('user', filters)
  },

  async getUsageByProvider(filters?: UsageFilters & { limit?: number }): Promise<UsageByProvider[]> {
    return this.getUsageAggregation<UsageByProvider[]>('provider', filters)
  },

  async getUsageByApiFormat(filters?: UsageFilters & { limit?: number }): Promise<UsageByApiFormat[]> {
    return this.getUsageAggregation<UsageByApiFormat[]>('api_format', filters)
  },

  async getUserUsage(userId: string, filters?: UsageFilters): Promise<{
    records: UsageRecord[]
    stats: UsageStats
  }> {
    const response = await apiClient.get(`/api/users/${userId}/usage`, { params: filters })
    return response.data
  },

  async exportUsage(format: 'csv' | 'json', filters?: UsageFilters): Promise<Blob> {
    const response = await apiClient.get('/api/usage/export', {
      params: { ...filters, format },
      responseType: 'blob'
    })
    return response.data
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
    api_format?: string  // API 格式筛选（如 openai:chat, claude:chat）
    status?: string // 'stream' | 'standard' | 'error'
    limit?: number
    offset?: number
  }): Promise<{
    records: Array<Record<string, unknown>>
    total: number
    limit: number
    offset: number
  }> {
    const key = buildCacheKey('usage:records', params as Record<string, unknown> | undefined)
    return dedupedRequest(key, async () => {
      const response = await apiClient.get('/api/admin/usage/records', { params })
      return response.data
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
      cache_read_input_tokens?: number | null
      cost: number
      actual_cost?: number | null
      rate_multiplier?: number | null
      response_time_ms: number | null
      first_byte_time_ms: number | null
      provider?: string | null
      api_key_name?: string | null
      provider_key_name?: string | null
      api_format?: string | null
      endpoint_api_format?: string | null
      has_format_conversion?: boolean | null
      target_model?: string | null
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
   * 后端已缓存5分钟
   */
  async getActivityHeatmap(): Promise<ActivityHeatmap> {
    return cachedRequest(
      'admin-usage-activity-heatmap',
      async () => {
        const response = await apiClient.get<ActivityHeatmap | unknown[]>('/api/admin/usage/heatmap')
        return normalizeActivityHeatmapResponse(response.data)
      },
      60000
    )
  }
}
