import type { ActivityHeatmap } from '@/types/activity'

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
  activity_heatmap: ActivityHeatmap | null
}

// 模型统计
export interface ModelStatsItem {
  model: string
  request_count: number
  total_tokens: number
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
  total_cost: number
  actual_cost?: number
  avgResponseTime: string
}

// 请求记录
// 请求状态类型
export type RequestStatus = 'pending' | 'streaming' | 'completed' | 'failed'

export interface UsageRecord {
  id: string
  user_id?: string
  username?: string
  user_email?: string
  provider: string
  api_key_name?: string
  rate_multiplier?: number
  model: string
  target_model?: string | null  // 映射后的目标模型名（若无映射则为空）
  api_format?: string
  input_tokens: number
  output_tokens: number
  cache_creation_input_tokens?: number
  cache_read_input_tokens?: number
  total_tokens: number
  cost: number
  actual_cost?: number
  response_time_ms?: number
  first_byte_time_ms?: number  // 首字时间 (TTFB)
  is_stream: boolean
  status_code?: number
  error_message?: string
  status?: RequestStatus  // 请求状态: pending, streaming, completed, failed
  created_at: string
  has_fallback?: boolean
  request_metadata?: {
    model_version?: string  // Provider 返回的实际模型版本（如 Gemini 的 modelVersion）
    [key: string]: unknown
  }
}

// 日期范围参数
export interface DateRangeParams {
  start_date?: string
  end_date?: string
}

// 时间段选项
export type PeriodValue = 'today' | 'yesterday' | 'last7days' | 'last30days' | 'last90days'

// 筛选状态（包含新的请求状态值）
export type FilterStatusValue = '__all__' | 'stream' | 'standard' | 'error' | 'active' | 'pending' | 'streaming' | 'completed' | 'failed'

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
    period_end: '',
    activity_heatmap: null
  }
}
