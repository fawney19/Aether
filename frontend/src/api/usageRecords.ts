import type { ImageProgress } from './requestTrace'

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
  request_type?: string | null  // 由请求语义识别出的操作类型
  requested_reasoning_effort?: string | null  // 用户请求侧 reasoning 级别，用于展示转换关系
  reasoning_effort?: string | null  // 从发送给 Provider 的请求体提取的 reasoning 级别
  service_tier?: string | null  // 从发送给 Provider 的请求体提取的服务层级
  actual_service_tier?: string | null  // 响应侧审计事实，不用于 Fast 展示或计费
  api_format?: string
  endpoint_api_format?: string  // 端点原生格式
  has_format_conversion?: boolean  // 是否发生了格式转换
  input_tokens: number
  effective_input_tokens?: number
  output_tokens: number
  reasoning_tokens?: number
  cache_creation_input_tokens?: number
  cache_creation_ephemeral_5m_input_tokens?: number
  cache_creation_ephemeral_1h_input_tokens?: number
  cache_read_input_tokens?: number
  total_tokens: number
  cost: number
  actual_cost?: number
  response_time_ms?: number | null
  first_byte_time_ms?: number | null  // 首字时间 (TTFB)
  end_to_end_time_ms?: number | null  // 客户端从请求进入网关到完成的总耗时
  end_to_end_first_byte_time_ms?: number | null  // 客户端从请求进入网关到首字节的耗时
  is_stream: boolean
  is_websocket?: boolean
  websocket_transport?: string | null
  usage_available?: boolean
  usage_pricing_available?: boolean
  input_audio_tokens?: number | null
  output_audio_tokens?: number | null
  upstream_is_stream?: boolean
  client_requested_stream?: boolean
  client_is_stream?: boolean
  client_family?: string | null
  client_ip?: string | null
  user_agent?: string | null
  request_path?: string | null
  request_path_and_query?: string | null
  status_code?: number
  error_message?: string
  status?: RequestStatus  // 请求状态: pending, streaming, completed, failed
  created_at: string
  updated_at?: string | null
  response_time_updated_at?: string | null
  has_fallback?: boolean
  has_retry?: boolean
  image_progress?: ImageProgress | null
}
