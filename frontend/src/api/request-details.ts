import apiClient from './client'

export interface VideoBilling {
  task_type: 'video' | 'image' | 'audio'
  duration_seconds?: number
  resolution?: string
  video_price_per_second?: number
  video_cost?: number
  cost?: number
  rule_name?: string
  expression?: string
  status?: string
}

export interface RequestDetail {
  id: string
  request_id: string
  user: {
    id: string | null
    username: string | null
    email: string | null
  }
  api_key: {
    id: string | null
    name: string | null
    display: string | null
  }
  provider_api_key: {
    id: string | null
    name: string | null
  }
  provider: string
  api_format?: string
  model: string
  target_model?: string | null
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
  input_tokens?: number
  output_tokens?: number
  total_tokens?: number
  cache_creation_input_tokens?: number
  cache_creation_input_tokens_5m?: number
  cache_creation_input_tokens_1h?: number
  cache_read_input_tokens?: number
  input_cost?: number
  output_cost?: number
  total_cost?: number
  cache_creation_cost?: number
  cache_read_cost?: number
  request_cost?: number
  input_price_per_1m?: number
  output_price_per_1m?: number
  cache_creation_price_per_1m?: number
  cache_read_price_per_1m?: number
  price_per_request?: number
  request_type: string
  is_stream: boolean
  status_code: number
  status?: string
  error_message?: string
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
  tiered_pricing?: {
    total_input_context: number
    tier_index: number
    tier_count: number
    source?: 'provider' | 'global'
    current_tier: {
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
    tiers: Array<{
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
}

export const requestDetailsApi = {
  async getRequestDetail(requestId: string, options: { includeBodies?: boolean } = {}): Promise<RequestDetail> {
    const response = await apiClient.get<RequestDetail>(`/api/admin/usage/${requestId}`, {
      params: { include_bodies: options.includeBodies ?? true },
    })
    return response.data
  },

  async getCurlData(requestId: string): Promise<CurlData> {
    const response = await apiClient.get<CurlData>(`/api/admin/usage/${requestId}/curl`)
    return response.data
  },

  async replayRequest(requestId: string, params?: ReplayRequest): Promise<ReplayResponse> {
    const response = await apiClient.post<ReplayResponse>(
      `/api/admin/usage/${requestId}/replay`,
      params || {},
    )
    return response.data
  },
}
