import apiClient from './client'
import { cachedRequest, buildCacheKey } from '@/utils/cache'

const REQUEST_DETAIL_PREFETCH_TTL_MS = 5_000

export interface DashboardStat {
  name: string
  value: string
  subValue?: string
  change?: string
  changeType?: 'increase' | 'decrease' | 'neutral'
  extraBadge?: string
  icon: string
}

export interface RecentRequest {
  id: string // UUID
  user: string
  model: string
  tokens: number
  time: string
}

export interface ProviderStatus {
  name: string
  status: 'active' | 'inactive'
  requests: number
}

// 系统健康指标（管理员专用）
export interface SystemHealth {
  avg_response_time: number
  error_rate: number
  error_requests: number
  fallback_count: number
  total_requests: number
}

// 成本统计（管理员专用）
export interface CostStats {
  total_cost: number
  total_actual_cost: number
  cost_savings: number
}

// 缓存统计
export interface CacheStats {
  cache_creation_tokens: number
  cache_read_tokens: number
  cache_creation_cost?: number
  cache_read_cost?: number
  cache_hit_rate?: number
  total_cache_tokens: number
}

// 用户统计（管理员专用）
export interface UserStats {
  total: number
  active: number
}

// Token 详细分类
export interface TokenBreakdown {
  input: number
  output: number
  cache_creation: number
  cache_read: number
}

export interface DashboardStatsResponse {
  stats: DashboardStat[]
  today?: {
    requests: number
    tokens: number
    cost: number
    actual_cost?: number
    cache_creation_tokens?: number
    cache_read_tokens?: number
  }
  api_keys?: {
    total: number
    active: number
  }
  tokens?: {
    month: number
  }
  // 管理员专用字段
  system_health?: SystemHealth
  cost_stats?: CostStats
  cache_stats?: CacheStats
  users?: UserStats
  token_breakdown?: TokenBreakdown
  // 普通用户专用字段
  monthly_cost?: number
}

export interface RecentRequestsResponse {
  requests: RecentRequest[]
}

export interface ProviderStatusResponse {
  providers: ProviderStatus[]
}

// 视频/图像/音频计费信息
export interface VideoBilling {
  task_type: 'video' | 'image' | 'audio'
  duration_seconds?: number  // 视频时长（秒）
  resolution?: string        // 分辨率
  video_price_per_second?: number  // 每秒单价
  video_cost?: number        // 视频费用
  cost?: number              // 总费用
  rule_name?: string         // 计费规则名称
  expression?: string        // 计费公式
  status?: string            // 计费状态
}

export interface RequestErrorDomain {
  source?: string | null
  status_code?: number | null
  type?: string | null
  message?: string | null
  code?: string | number | null
  content_type?: string | null
  body?: unknown
  category?: string | null
}

export interface RequestErrorDomains {
  request_error?: RequestErrorDomain | null
  upstream_error?: RequestErrorDomain | null
  client_error?: RequestErrorDomain | null
  failure_summary?: RequestErrorDomain | null
}

export interface RequestErrorFlow {
  source?: string | null
  status_code?: number | null
  propagation?: string | null
  client_response_source?: string | null
  safe_to_expose_upstream?: boolean | null
  summary_source?: string | null
}

export interface RequestDetail {
  id: string // UUID
  request_id: string
  user: {
    id: string // UUID
    username: string
    email: string
  }
  api_key: {
    id: string // UUID
    name: string
    display: string
  }
  provider: string
  api_format?: string
  endpoint_api_format?: string
  model: string
  target_model?: string | null  // 映射后的目标模型名
  tokens: {
    input: number
    output: number
    total: number
  }
  cost: {
    input: number
    output: number
    total: number
  }
  // Additional token fields
  input_tokens?: number
  effective_input_tokens?: number
  output_tokens?: number
  total_tokens?: number
  cache_creation_input_tokens?: number
  cache_creation_input_tokens_5m?: number
  cache_creation_input_tokens_1h?: number
  cache_read_input_tokens?: number
  // Additional cost fields
  input_cost?: number
  output_cost?: number
  total_cost?: number
  cache_creation_cost?: number
  cache_read_cost?: number
  request_cost?: number  // 按次计费费用
  // Historical pricing fields (per 1M tokens)
  input_price_per_1m?: number
  output_price_per_1m?: number
  cache_creation_price_per_1m?: number
  cache_read_price_per_1m?: number
  price_per_request?: number  // 按次计费价格
  request_type: string
  is_stream: boolean
  upstream_is_stream?: boolean
  client_requested_stream?: boolean
  client_is_stream?: boolean
  status_code: number
  status?: string  // pending, streaming, completed, failed, cancelled
  error_message?: string
  request_error?: RequestErrorDomain | null
  upstream_error?: RequestErrorDomain | null
  client_error?: RequestErrorDomain | null
  failure_summary?: RequestErrorDomain | null
  errors?: RequestErrorDomains | null
  error_flow?: RequestErrorFlow | null
  response_time_ms: number
  created_at: string
  request_headers?: Record<string, unknown>
  request_body?: Record<string, unknown>
  provider_request_headers?: Record<string, unknown>
  provider_request_body?: Record<string, unknown>
  response_headers?: Record<string, unknown>
  client_response_headers?: Record<string, unknown>
  response_body?: Record<string, unknown>
  client_response_body?: Record<string, unknown>
  has_request_body?: boolean
  has_provider_request_body?: boolean
  has_response_body?: boolean
  has_client_response_body?: boolean
  metadata?: Record<string, unknown>
  routing?: Record<string, unknown>
  body_capture?: Record<string, unknown>
  trace?: Record<string, unknown>
  settlement?: {
    billing_snapshot?: Record<string, unknown>
    billing_snapshot_schema_version?: string
    billing_snapshot_status?: string
    rate_multiplier?: number
    is_free_tier?: boolean
    input_price_per_1m?: number
    output_price_per_1m?: number
    cache_creation_price_per_1m?: number
    cache_read_price_per_1m?: number
    price_per_request?: number
  } | null
  // 阶梯计费信息
  tiered_pricing?: {
    total_input_context: number  // 总输入上下文 (input + cache_read)
    tier_index: number  // 命中的阶梯索引 (0-based)
    tier_count: number  // 阶梯总数
    source?: 'provider' | 'global'  // 定价来源: 提供商或全局
    current_tier: {  // 当前命中的阶梯配置
      up_to?: number | null
      input_price_per_1m: number
      output_price_per_1m: number
      cache_creation_price_per_1m?: number
      cache_read_price_per_1m?: number
      cache_ttl_pricing?: Array<{
        ttl_minutes: number
        cache_creation_price_per_1m?: number
        cache_read_price_per_1m?: number
      }>
    }
    tiers: Array<{  // 完整阶梯配置列表
      up_to?: number | null
      input_price_per_1m: number
      output_price_per_1m: number
      cache_creation_price_per_1m?: number
      cache_read_price_per_1m?: number
      cache_ttl_pricing?: Array<{
        ttl_minutes: number
        cache_creation_price_per_1m?: number
        cache_read_price_per_1m?: number
      }>
    }>
  } | null
  // 视频/图像/音频计费信息
  video_billing?: VideoBilling | null
}

export interface CurlData {
  url: string
  method: string
  headers: Record<string, string>
  body: Record<string, unknown>
  curl: string
}

export interface ReplayRequest {
  provider_id?: string
  endpoint_id?: string
  api_key_id?: string
  body_override?: Record<string, unknown>
}

export interface ReplayResponse {
  url: string
  provider: string
  status_code: number
  response_headers: Record<string, string>
  response_body: Record<string, unknown>
  response_time_ms: number
  mapping?: {
    source_model: string
    original_target_model?: string | null
    resolved_model: string
    target_provider_id: string
    target_provider: string
    target_endpoint_id: string
    target_api_format: string
    replay_mode: string
    mapping_applied: boolean
    mapping_source: string
  }
}

export interface ModelBreakdown {
  model: string
  requests: number
  tokens: number
  cost: number
}

export interface ModelSummary {
  model: string
  requests: number
  tokens: number
  cost: number
  avg_response_time: number
  cost_per_request: number
  tokens_per_request: number
}

export interface ProviderSummary {
  provider: string
  requests: number
  tokens: number
  cost: number
}

export interface DailyStat {
  date: string // ISO date string
  requests: number
  tokens: number
  cost: number
  avg_response_time: number // in seconds
  unique_models: number
  unique_providers?: number // 仅管理员返回
  model_breakdown: ModelBreakdown[]
}

export interface DailyStatsResponse {
  daily_stats: DailyStat[]
  model_summary: ModelSummary[]
  provider_summary?: ProviderSummary[] // 仅管理员返回
  period: {
    start_date: string
    end_date: string
    days: number
  }
}

export interface TimeRangeParams {
  start_date?: string
  end_date?: string
  preset?: string
  granularity?: 'hour' | 'day' | 'week' | 'month'
  timezone?: string
  tz_offset_minutes?: number
}

export const dashboardApi = {
  // 获取仪表盘统计数据
  async getStats(params?: TimeRangeParams): Promise<DashboardStatsResponse> {
    const cacheKey = buildCacheKey('dashboard:stats', params)
    return cachedRequest(
      cacheKey,
      async () => {
        const response = await apiClient.get<DashboardStatsResponse>('/api/dashboard/stats', { params })
        return response.data
      },
      10 * 1000
    )
  },

  // 获取最近的请求记录
  async getRecentRequests(limit: number = 10): Promise<RecentRequest[]> {
    const response = await apiClient.get<RecentRequestsResponse>('/api/dashboard/recent-requests', {
      params: { limit }
    })
    return response.data.requests
  },

  // 获取提供商状态
  async getProviderStatus(): Promise<ProviderStatus[]> {
    return cachedRequest(
      'dashboard:provider-status',
      async () => {
        const response = await apiClient.get<ProviderStatusResponse>('/api/dashboard/provider-status')
        return response.data.providers
      },
      20 * 1000
    )
  },

  // 获取请求详情
  // NOTE: This method now calls the new RESTful API at /api/admin/usage/{id}
  async getRequestDetail(
    requestId: string,
    options: { includeBodies?: boolean, cacheTtlMs?: number } = {}
  ): Promise<RequestDetail> {
    const includeBodies = options.includeBodies ?? true
    const cacheTtlMs = options.cacheTtlMs ?? 0
    const cacheKey = buildCacheKey('dashboard:request-detail', { requestId, includeBodies })
    return cachedRequest(
      cacheKey,
      async () => {
        const response = await apiClient.get<RequestDetail>(`/api/admin/usage/${requestId}`, {
          params: { include_bodies: includeBodies },
        })
        return response.data
      },
      cacheTtlMs
    )
  },

  async prefetchRequestDetail(requestId: string): Promise<void> {
    await dashboardApi.getRequestDetail(requestId, {
      includeBodies: false,
      cacheTtlMs: REQUEST_DETAIL_PREFETCH_TTL_MS
    })
  },

  // 获取每日统计数据
  async getDailyStats(params?: TimeRangeParams & { days?: number }): Promise<DailyStatsResponse> {
    const cacheKey = buildCacheKey('dashboard:daily-stats', params)
    return cachedRequest(
      cacheKey,
      async () => {
        const response = await apiClient.get<DailyStatsResponse>('/api/dashboard/daily-stats', {
          params
        })
        return response.data
      },
      20 * 1000
    )
  },

  // 获取 cURL 命令数据（含明文 API Key）
  async getCurlData(requestId: string): Promise<CurlData> {
    const response = await apiClient.get<CurlData>(`/api/admin/usage/${requestId}/curl`)
    return response.data
  },

  // 回放请求到提供商
  async replayRequest(requestId: string, params?: ReplayRequest): Promise<ReplayResponse> {
    const response = await apiClient.post<ReplayResponse>(
      `/api/admin/usage/${requestId}/replay`,
      params || {}
    )
    return response.data
  }
}
