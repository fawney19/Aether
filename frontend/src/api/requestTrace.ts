import apiClient from './client'

export interface CandidateRecord {
  id: string
  request_id: string
  candidate_index: number
  retry_index: number
  provider_id?: string
  provider_name?: string
  provider_website?: string  // Provider 官网
  endpoint_id?: string
  endpoint_name?: string  // 端点显示名称（api_format）
  key_id?: string
  key_name?: string  // 密钥名称
  key_preview?: string  // 密钥脱敏预览（如 sk-***abc），OAuth 类型不返回
  key_auth_type?: string  // 密钥认证类型（api_key, service_account, oauth 等）
  key_oauth_plan_type?: string  // OAuth 账号套餐类型（free/plus/team/enterprise）
  key_capabilities?: Record<string, boolean> | null  // Key 支持的能力
  required_capabilities?: Record<string, boolean> | null  // 请求实际需要的能力标签
  status: 'pending' | 'streaming' | 'success' | 'failed' | 'skipped' | 'cancelled' | 'available' | 'unused' | 'stream_interrupted'
  skip_reason?: string
  is_cached: boolean
  // 执行结果字段
  status_code?: number
  error_type?: string
  error_message?: string
  latency_ms?: number
  concurrent_requests?: number
  extra_data?: Record<string, unknown>
  created_at: string
  started_at?: string
  finished_at?: string
}

export interface RequestTrace {
  request_id: string
  total_candidates: number
  final_status: 'success' | 'failed' | 'streaming' | 'pending' | 'cancelled'
  total_latency_ms: number
  candidates: CandidateRecord[]
}

export interface ProviderStats {
  total_attempts: number
  success_count: number
  failed_count: number
  cancelled_count: number
  skipped_count: number
  pending_count: number
  available_count: number
  failure_rate: number
}

export const requestTraceApi = {
  /**
   * 获取特定请求的完整追踪信息
   */
  async getRequestTrace(requestId: string): Promise<RequestTrace> {
    const response = await apiClient.get<RequestTrace>(`/api/admin/monitoring/trace/${requestId}`)
    return response.data
  },

  /**
   * 获取某个 Provider 的失败率统计
   */
  async getProviderStats(providerId: string, limit: number = 100): Promise<ProviderStats> {
    const response = await apiClient.get<ProviderStats>(`/api/admin/monitoring/trace/stats/provider/${providerId}`, {
      params: { limit }
    })
    return response.data
  }
}
