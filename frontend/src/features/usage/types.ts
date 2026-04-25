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
export type RequestStatus = 'pending' | 'streaming' | 'completed' | 'failed' | 'cancelled'

export interface UsageRecord {
  id: string
  user_id?: string
  username?: string
  user_email?: string
  api_key?: {
    id: string | null
    name: string | null
    display: string | null
  } | null
  provider?: string  // 仅管理员可见
  api_key_name?: string
  provider_key_name?: string | null
  rate_multiplier?: number
  model: string
  target_model?: string | null  // 映射后的目标模型名（若无映射则为空）
  model_version?: string | null  // Provider 返回的实际模型版本（列表轻量字段）
  api_format?: string
  endpoint_api_format?: string  // 端点原生格式
  has_format_conversion?: boolean  // 是否发生了格式转换
  input_tokens: number
  effective_input_tokens?: number
  output_tokens: number
  cache_creation_input_tokens?: number
  cache_creation_ephemeral_5m_input_tokens?: number
  cache_creation_ephemeral_1h_input_tokens?: number
  cache_read_input_tokens?: number
  total_tokens: number
  cost: number
  actual_cost?: number
  response_time_ms?: number
  first_byte_time_ms?: number  // 首字时间 (TTFB)
  is_stream: boolean
  upstream_is_stream?: boolean
  client_requested_stream?: boolean
  client_is_stream?: boolean
  status_code?: number
  error_message?: string
  status?: RequestStatus  // 请求状态: pending, streaming, completed, failed
  created_at: string
  has_fallback?: boolean
  has_retry?: boolean
}

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
  'active' |
  'failed' |
  'cancelled' |
  'has_fallback'

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
