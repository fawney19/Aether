import apiClient from './client'
import { cachedRequest, buildCacheKey } from '@/utils/cache'
import type { ActivityHeatmap } from '@/types/activity'

export type AnalyticsScopeKind = 'global' | 'me' | 'user' | 'api_key'
export type AnalyticsGranularity = 'hour' | 'day' | 'week' | 'month'
export type AnalyticsBreakdownDimension = 'model' | 'provider' | 'api_format' | 'api_key' | 'user'
export type AnalyticsBreakdownMetric = 'requests_total' | 'total_tokens' | 'total_cost_usd' | 'actual_total_cost_usd'
export type AnalyticsLeaderboardEntity = 'user' | 'api_key'
export type AnalyticsLeaderboardMetric = 'requests_total' | 'total_tokens' | 'total_cost_usd'

export interface AnalyticsScope {
  kind: AnalyticsScopeKind
  user_id?: string | null
  api_key_id?: string | null
}

export interface AnalyticsTimeRange {
  start_date?: string
  end_date?: string
  preset?: string
  granularity?: AnalyticsGranularity
  timezone?: string | null
  tz_offset_minutes?: number
}

export interface AnalyticsFilters {
  user_ids?: string[]
  provider_names?: string[]
  models?: string[]
  target_models?: string[]
  api_key_ids?: string[]
  api_formats?: string[]
  request_types?: string[]
  statuses?: string[]
  error_categories?: string[]
  is_stream?: boolean | null
  has_format_conversion?: boolean | null
}

export interface AnalyticsBaseRequest {
  scope: AnalyticsScope
  time_range: AnalyticsTimeRange
  filters?: AnalyticsFilters
}

export interface AnalyticsSummary {
  requests_total: number
  requests_success: number
  requests_error: number
  requests_stream: number
  success_rate: number
  input_tokens: number
  output_tokens: number
  input_output_total_tokens: number
  cache_creation_input_tokens: number
  cache_creation_input_tokens_5m: number
  cache_creation_input_tokens_1h: number
  cache_read_input_tokens: number
  input_context_tokens: number
  total_tokens: number
  cache_hit_rate: number
  input_cost_usd: number
  output_cost_usd: number
  cache_creation_cost_usd: number
  cache_creation_cost_usd_5m: number
  cache_creation_cost_usd_1h: number
  cache_read_cost_usd: number
  cache_cost_usd: number
  request_cost_usd: number
  total_cost_usd: number
  actual_total_cost_usd: number
  actual_cache_cost_usd: number
  avg_response_time_ms: number
  avg_first_byte_time_ms: number
  format_conversion_count: number
  models_used_count: number
}

export interface AnalyticsCompositionSegment {
  key: string
  value: number
  percentage: number
}

export interface AnalyticsOverviewResponse {
  query_context: {
    scope: AnalyticsScope
    time_range: AnalyticsTimeRange
  }
  summary: AnalyticsSummary
  composition: {
    token_segments: AnalyticsCompositionSegment[]
    cost_segments: AnalyticsCompositionSegment[]
  }
}

export interface AnalyticsTimeseriesBucket extends AnalyticsSummary {
  bucket_start: string
  bucket_end: string
}

export interface AnalyticsTimeseriesResponse {
  buckets: AnalyticsTimeseriesBucket[]
}

export interface AnalyticsBreakdownRequest extends AnalyticsBaseRequest {
  dimension: AnalyticsBreakdownDimension
  metric?: AnalyticsBreakdownMetric
  limit?: number
}

export interface AnalyticsBreakdownRow extends AnalyticsSummary {
  key: string
  label: string
  share_of_total_cost: number
  share_of_total_tokens: number
}

export interface AnalyticsBreakdownResponse {
  dimension: AnalyticsBreakdownDimension
  metric: AnalyticsBreakdownMetric
  rows: AnalyticsBreakdownRow[]
}

export interface AnalyticsRecordsRequest extends AnalyticsBaseRequest {
  search?: {
    text?: string | null
    request_id?: string | null
  }
  pagination?: {
    limit?: number
    offset?: number
  }
}

export interface AnalyticsRecord {
  id: string
  request_id: string
  created_at: string | null
  user_id: string | null
  username: string | null
  api_key_id: string | null
  api_key_name: string | null
  provider_api_key_name: string | null
  provider_name: string | null
  model: string
  target_model: string | null
  api_format: string | null
  request_type: string | null
  status: string
  billing_status: string
  is_stream: boolean
  has_format_conversion: boolean | null
  has_fallback?: boolean
  has_retry?: boolean
  status_code: number | null
  error_message: string | null
  error_category: string | null
  response_time_ms: number | null
  first_byte_time_ms: number | null
  input_tokens: number
  output_tokens: number
  input_output_total_tokens: number
  cache_creation_input_tokens: number
  cache_creation_input_tokens_5m: number
  cache_creation_input_tokens_1h: number
  cache_read_input_tokens: number
  input_context_tokens: number
  total_tokens: number
  input_cost_usd: number
  output_cost_usd: number
  cache_creation_cost_usd: number
  cache_creation_cost_usd_5m: number
  cache_creation_cost_usd_1h: number
  cache_read_cost_usd: number
  cache_cost_usd: number
  request_cost_usd: number
  total_cost_usd: number
  actual_total_cost_usd: number
  actual_cache_cost_usd: number
  rate_multiplier: number
}

export interface AnalyticsRecordsResponse {
  total: number
  limit: number
  offset: number
  records: AnalyticsRecord[]
}

export interface AnalyticsFilterOption {
  value: string
  label: string
}

export interface AnalyticsFilterOptionsResponse {
  providers: AnalyticsFilterOption[]
  models: AnalyticsFilterOption[]
  target_models: AnalyticsFilterOption[]
  api_formats: AnalyticsFilterOption[]
  request_types: AnalyticsFilterOption[]
  error_categories: AnalyticsFilterOption[]
  statuses: AnalyticsFilterOption[]
  users?: AnalyticsFilterOption[]
  api_keys?: AnalyticsFilterOption[]
}

export interface AnalyticsLeaderboardRequest extends AnalyticsBaseRequest {
  entity: AnalyticsLeaderboardEntity
  metric: AnalyticsLeaderboardMetric
  limit?: number
}

export interface AnalyticsLeaderboardItem {
  rank: number
  id: string
  label: string
  requests_total: number
  total_tokens: number
  total_cost_usd: number
  actual_total_cost_usd: number
  metric_value: number
}

export interface AnalyticsLeaderboardResponse {
  entity: AnalyticsLeaderboardEntity
  metric: AnalyticsLeaderboardMetric
  items: AnalyticsLeaderboardItem[]
}

export interface AnalyticsPercentilePoint {
  date: string
  p50_response_time_ms: number | null
  p90_response_time_ms: number | null
  p99_response_time_ms: number | null
  p50_first_byte_time_ms: number | null
  p90_first_byte_time_ms: number | null
  p99_first_byte_time_ms: number | null
}

export interface AnalyticsErrorCategory {
  category: string
  label: string
  count: number
}

export interface AnalyticsErrorTrendItem {
  date: string
  total: number
}

export interface AnalyticsProviderHealthItem {
  provider_name: string
  requests_total: number
  success_rate: number
  error_rate: number
  avg_response_time_ms: number
  avg_first_byte_time_ms: number
}

export interface AnalyticsPerformanceResponse {
  latency: {
    response_time_ms: {
      avg: number
      p50: number | null
      p90: number | null
      p99: number | null
    }
    first_byte_time_ms: {
      avg: number
      p50: number | null
      p90: number | null
      p99: number | null
    }
  }
  percentiles: AnalyticsPercentilePoint[]
  errors: {
    total: number
    rate: number
    categories: AnalyticsErrorCategory[]
    trend: AnalyticsErrorTrendItem[]
  }
  provider_health: AnalyticsProviderHealthItem[]
}

export interface AnalyticsActiveRequest {
  id: string
  status: 'pending' | 'streaming' | 'completed' | 'failed' | 'cancelled'
  input_tokens: number
  output_tokens: number
  cache_creation_input_tokens?: number | null
  cache_read_input_tokens?: number | null
  cost?: number
  total_cost_usd?: number
  actual_cost?: number | null
  actual_total_cost_usd?: number | null
  rate_multiplier?: number | null
  response_time_ms: number | null
  first_byte_time_ms: number | null
  provider?: string | null
  provider_name?: string | null
  api_key_name?: string | null
  api_format?: string | null
  endpoint_api_format?: string | null
  has_format_conversion?: boolean | null
  target_model?: string | null
}

export interface AnalyticsActiveRequestsResponse {
  requests: AnalyticsActiveRequest[]
}

export const analyticsApi = {
  async getOverview(payload: AnalyticsBaseRequest): Promise<AnalyticsOverviewResponse> {
    const response = await apiClient.post<AnalyticsOverviewResponse>('/api/analytics/overview', payload)
    return response.data
  },

  async getTimeseries(payload: AnalyticsBaseRequest): Promise<AnalyticsTimeseriesResponse> {
    const response = await apiClient.post<AnalyticsTimeseriesResponse>('/api/analytics/timeseries', payload)
    return response.data
  },

  async getBreakdown(payload: AnalyticsBreakdownRequest): Promise<AnalyticsBreakdownResponse> {
    const response = await apiClient.post<AnalyticsBreakdownResponse>('/api/analytics/breakdown', payload)
    return response.data
  },

  async getRecords(payload: AnalyticsRecordsRequest): Promise<AnalyticsRecordsResponse> {
    const response = await apiClient.post<AnalyticsRecordsResponse>('/api/analytics/records', payload)
    return response.data
  },

  async getFilterOptions(payload: AnalyticsBaseRequest): Promise<AnalyticsFilterOptionsResponse> {
    const response = await apiClient.post<AnalyticsFilterOptionsResponse>('/api/analytics/filter-options', payload)
    return response.data
  },

  async getLeaderboard(payload: AnalyticsLeaderboardRequest): Promise<AnalyticsLeaderboardResponse> {
    const response = await apiClient.post<AnalyticsLeaderboardResponse>('/api/analytics/leaderboard', payload)
    return response.data
  },

  async getPerformance(payload: AnalyticsBaseRequest): Promise<AnalyticsPerformanceResponse> {
    const response = await apiClient.post<AnalyticsPerformanceResponse>('/api/analytics/performance', payload)
    return response.data
  },

  async getHeatmap(payload: {
    scope: AnalyticsScope
    user_id?: string | null
    api_key_id?: string | null
  }): Promise<ActivityHeatmap> {
    const cacheKey = buildCacheKey('analytics:heatmap', payload as Record<string, unknown>)
    return cachedRequest(
      cacheKey,
      async () => {
        const response = await apiClient.post<ActivityHeatmap>('/api/analytics/heatmap', payload)
        return response.data
      },
      60000,
    )
  },

  async getActiveRequests(payload: {
    scope: AnalyticsScope
    ids?: string[]
  }): Promise<AnalyticsActiveRequestsResponse> {
    const response = await apiClient.post<AnalyticsActiveRequestsResponse>('/api/analytics/active-requests', {
      ...payload,
      ids: payload.ids ?? [],
    })
    return response.data
  },

  async analyzeCacheAffinityTTL(payload: {
    scope: AnalyticsScope
    user_id?: string | null
    api_key_id?: string | null
    hours?: number
  }): Promise<Record<string, unknown>> {
    const response = await apiClient.post('/api/analytics/cache-affinity/ttl-analysis', payload)
    return response.data
  },

  async analyzeCacheAffinityHit(payload: {
    scope: AnalyticsScope
    user_id?: string | null
    api_key_id?: string | null
    hours?: number
  }): Promise<Record<string, unknown>> {
    const response = await apiClient.post('/api/analytics/cache-affinity/hit-analysis', payload)
    return response.data
  },
}
