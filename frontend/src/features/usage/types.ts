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
  api_key_name?: string  // 提供商 Key 名称（管理员列使用）
  rate_multiplier?: number
  model: string
  target_model?: string | null  // 映射后的目标模型名（若无映射则为空）
  model_version?: string | null  // Provider 返回的实际模型版本（列表轻量字段）
  api_format?: string
  endpoint_api_format?: string  // 端点原生格式
  has_format_conversion?: boolean  // 是否发生了格式转换
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
  has_retry?: boolean
}

// 日期范围参数
export interface DateRangeParams {
  start_date?: string
  end_date?: string
  preset?: string
  granularity?: 'auto' | 'hour' | 'day' | 'week' | 'month'
  timezone?: string
  tz_offset_minutes?: number
}

// 时间段选项
export type PeriodValue = 'today' | 'last7days' | 'last30days' | 'last180days' | 'last1year'

// 筛选状态由后端 filter-options 决定
export type FilterStatusValue = string
