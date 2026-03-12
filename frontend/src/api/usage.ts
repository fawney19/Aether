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
  async getActiveRequests(ids?: string[]): Promise<{
    requests: Array<{
      id: string
      status: 'pending' | 'streaming' | 'completed' | 'failed' | 'cancelled'
      input_tokens: number
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
      api_format?: string | null
      endpoint_api_format?: string | null
      has_format_conversion?: boolean | null
      target_model?: string | null
    }>
  }> {
    const params = ids?.length ? { ids: ids.join(',') } : {}
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
        const response = await apiClient.get<ActivityHeatmap>('/api/admin/usage/heatmap')
        return response.data
      },
      60000
    )
  }
}
