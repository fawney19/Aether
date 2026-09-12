export type { UsageRecord, RequestStatus } from '@/api/usageRecords'

// 统计数据状态
export interface UsageStatsState {
  total_requests: number
  total_tokens: number
  total_cost: number
  total_actual_cost?: number  // 倍率消耗（仅管理员可见）
  avg_response_time: number
  error_count?: number
  error_rate?: number
  cache_stats?: {
    cache_creation_tokens: number
    cache_read_tokens: number
    cache_creation_cost: number
    cache_read_cost: number
  }
  period_start: string
  period_end: string
}

// 模型统计
export interface ModelStatsItem {
  model: string
  request_count: number
  total_tokens: number
  effective_input_tokens?: number
  total_input_context?: number
  output_tokens?: number
  cache_read_tokens?: number
  cache_creation_tokens?: number
  cache_hit_rate?: number
  total_cost: number
  actual_cost?: number  // 倍率消耗
}

// 增强的模型统计（包含效率分析）
export interface EnhancedModelStatsItem extends ModelStatsItem {
  costPerToken: string
}

// 提供商统计
export interface ProviderStatsItem {
  providerId?: string | null
  providerKey?: string
  providerIdentitySource?: 'provider_id' | 'legacy_name'
  provider: string
  requests: number
  totalTokens: number
  effectiveInputTokens?: number
  totalInputContext?: number
  outputTokens?: number
  cacheReadTokens?: number
  cacheCreationTokens?: number
  cacheHitRate?: number
  totalCost: number
  actualCost?: number
  successRate: number
  avgResponseTime: string
}

// API格式统计
export interface ApiFormatStatsItem {
  api_format: string
  request_count: number
  total_tokens: number
  effective_input_tokens?: number
  total_input_context?: number
  output_tokens?: number
  cache_read_tokens?: number
  cache_creation_tokens?: number
  cache_hit_rate?: number
  total_cost: number
  actual_cost?: number
  avgResponseTime: string
}

// 请求记录
// 请求状态类型
// 日期范围参数
export interface DateRangeParams {
  start_date?: string
  end_date?: string
  preset?: string
  granularity?: 'hour' | 'day' | 'week' | 'month'
  timezone?: string
  tz_offset_minutes?: number
}

// 时间段选项
export type PeriodValue = 'today' | 'yesterday' | 'last7days' | 'last30days' | 'last90days'

// 筛选状态（简化为常用维度）
export type FilterStatusValue =
  '__all__' |
  'stream' |
  'standard' |
  'websocket' |
  'active' |
  'failed' |
  'cancelled' |
  'has_fallback' |
  'has_retry'

// 默认统计状态
export function createDefaultStats(): UsageStatsState {
  return {
    total_requests: 0,
    total_tokens: 0,
    total_cost: 0,
    total_actual_cost: undefined,
    avg_response_time: 0,
    error_count: undefined,
    error_rate: undefined,
    cache_stats: undefined,
    period_start: '',
    period_end: ''
  }
}
