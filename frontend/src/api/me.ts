import apiClient from './client'
import type { ActivityHeatmap } from '@/types/activity'
import type { TieredPricingConfig } from './endpoints/types'
import { cachedRequest, buildCacheKey } from '@/utils/cache'
import type { BillingSummary } from './auth'
import type { UserSession } from '@/types/session'

export type { UserSession }

export interface Profile {
  id: string // UUID
  email?: string | null
  username: string
  role: string
  is_active: boolean
  billing: BillingSummary
  created_at: string
  updated_at?: string
  last_login_at?: string
  auth_source?: 'local' | 'ldap' | 'oauth'
  has_password?: boolean
  preferences?: UserPreferences
}

export interface UserPreferences {
  avatar_url?: string
  bio?: string
  default_provider_id?: string // UUID
  default_provider?: Record<string, unknown>
  theme: string
  language: string
  timezone?: string
  notifications?: {
    email?: boolean
    usage_alerts?: boolean
    announcements?: boolean
  }
}

// 提供商配置接口
export interface ProviderConfig {
  provider_id: string
  priority: number  // 优先级（越高越优先）
  weight: number    // 负载均衡权重
  enabled: boolean  // 是否启用
}

// 使用记录接口
export interface UsageRecordDetail {
  id: string
  provider: string
  model: string
  input_tokens: number
  output_tokens: number
  total_tokens: number
  cost: number  // 官方费率
  actual_cost?: number  // 倍率消耗（仅管理员可见）
  rate_multiplier?: number  // 成本倍率（仅管理员可见）
  response_time_ms?: number
  is_stream: boolean
  created_at: string
  cache_creation_input_tokens?: number
  cache_read_input_tokens?: number
  status_code: number
  error_message?: string
  input_price_per_1m: number
  output_price_per_1m: number
  cache_creation_price_per_1m?: number
  cache_read_price_per_1m?: number
  price_per_request?: number  // 按次计费价格
  api_key?: {
    id: string
    name: string
    display: string
  }
}

// 模型统计接口
export interface ModelSummary {
  model: string
  requests: number
  input_tokens: number
  output_tokens: number
  total_tokens: number
  cache_read_tokens?: number
  cache_hit_rate?: number
  total_cost_usd: number
  actual_total_cost_usd?: number  // 倍率消耗（仅管理员可见）
}

// 提供商统计接口
export interface ProviderSummary {
  provider: string
  requests: number
  total_tokens: number
  cache_read_tokens?: number
  cache_hit_rate?: number
  total_cost_usd: number
  success_rate: number | null
  avg_response_time_ms: number | null
}

// API 格式统计接口
export interface ApiFormatSummary {
  api_format: string
  request_count: number
  total_tokens: number
  cache_read_tokens: number
  cache_hit_rate: number
  total_cost_usd: number
  avg_response_time_ms: number
}

// 使用统计响应接口
export interface UsageResponse {
  total_requests: number
  total_input_tokens: number
  total_output_tokens: number
  total_tokens: number
  total_cost: number  // 官方费率
  total_actual_cost?: number  // 倍率消耗（仅管理员可见）
  avg_response_time: number
  billing: BillingSummary
  summary_by_model: ModelSummary[]
  summary_by_provider?: ProviderSummary[]
  summary_by_api_format?: ApiFormatSummary[]
  pagination?: {
    total: number
    limit: number
    offset: number
    has_more: boolean
  }
  records: UsageRecordDetail[]
  activity_heatmap?: ActivityHeatmap | null
}

export interface ApiKey {
  id: string // UUID
  name: string
  key?: string
  key_display: string
  is_active: boolean
  is_locked: boolean  // 管理员锁定标志
  last_used_at?: string
  created_at: string
  total_requests?: number
  total_cost_usd?: number
  rate_limit?: number | null
  allowed_providers?: ProviderConfig[]
  force_capabilities?: Record<string, boolean> | null  // 强制能力配置
}

// 不再需要 ProviderBinding 接口

export interface ChangePasswordRequest {
  old_password?: string  // 可选：首次设置密码时不需要
  new_password: string
}

export const meApi = {
  // 获取个人信息
  async getProfile(): Promise<Profile> {
    const response = await apiClient.get<Profile>('/api/users/me')
    return response.data
  },

  // 更新个人信息
  async updateProfile(data: {
    email?: string
    username?: string
  }): Promise<{ message: string }> {
    const response = await apiClient.put('/api/users/me', data)
    return response.data
  },

  // 修改密码
  async changePassword(data: ChangePasswordRequest): Promise<{ message: string }> {
    const response = await apiClient.patch('/api/users/me/password', data)
    return response.data
  },

  async listSessions(): Promise<UserSession[]> {
    const response = await apiClient.get<UserSession[]>('/api/users/me/sessions')
    return response.data
  },

  async updateSessionLabel(sessionId: string, deviceLabel: string): Promise<UserSession> {
    const response = await apiClient.patch<UserSession>(`/api/users/me/sessions/${sessionId}`, {
      device_label: deviceLabel,
    })
    return response.data
  },

  async revokeSession(sessionId: string): Promise<{ message: string }> {
    const response = await apiClient.delete(`/api/users/me/sessions/${sessionId}`)
    return response.data
  },

  async revokeOtherSessions(): Promise<{ message: string; revoked_count: number }> {
    const response = await apiClient.delete('/api/users/me/sessions/others')
    return response.data
  },

  // API密钥管理
  async getApiKeys(): Promise<ApiKey[]> {
    const response = await apiClient.get<ApiKey[]>('/api/users/me/api-keys')
    return response.data
  },

  async createApiKey(data: { name: string; rate_limit?: number }): Promise<ApiKey> {
    const response = await apiClient.post<ApiKey>('/api/users/me/api-keys', data)
    return response.data
  },

  async getApiKeyDetail(keyId: string, includeKey: boolean = false): Promise<ApiKey & { key?: string }> {
    const response = await apiClient.get<ApiKey & { key?: string }>(
      `/api/users/me/api-keys/${keyId}`,
      { params: { include_key: includeKey } }
    )
    return response.data
  },

  async getFullApiKey(keyId: string): Promise<{ key: string }> {
    const response = await apiClient.get<{ key: string }>(
      `/api/users/me/api-keys/${keyId}`,
      { params: { include_key: true } }
    )
    return response.data
  },

  async deleteApiKey(keyId: string): Promise<{ message: string }> {
    const response = await apiClient.delete(`/api/users/me/api-keys/${keyId}`)
    return response.data
  },

  async toggleApiKey(keyId: string): Promise<ApiKey> {
    const response = await apiClient.patch<ApiKey>(`/api/users/me/api-keys/${keyId}`)
    return response.data
  },

  async updateApiKey(
    keyId: string,
    data: { name?: string; rate_limit?: number | null }
  ): Promise<ApiKey & { message: string }> {
    const response = await apiClient.put<ApiKey & { message: string }>(
      `/api/users/me/api-keys/${keyId}`,
      data
    )
    return response.data
  },

  // 使用统计
  async getUsage(params?: {
    start_date?: string
    end_date?: string
    preset?: string
    timezone?: string
    tz_offset_minutes?: number
    search?: string  // 通用搜索：密钥名、模型名
    limit?: number
    offset?: number
  }): Promise<UsageResponse> {
    const response = await apiClient.get<UsageResponse>('/api/users/me/usage', { params })
    return response.data
  },

  // 获取活跃请求状态（用于轮询更新）
  async getActiveRequests(ids?: string): Promise<{
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
      api_format?: string | null
      endpoint_api_format?: string | null
      has_format_conversion?: boolean | null
    }>
  }> {
    const params = ids ? { ids } : {}
    const response = await apiClient.get('/api/users/me/usage/active', { params })
    return response.data
  },

  // 获取可用的提供商
  async getAvailableProviders(): Promise<Array<Record<string, unknown>>> {
    const response = await apiClient.get('/api/users/me/providers')
    return response.data
  },

  // 获取用户可用的模型列表
  async getAvailableModels(params?: {
    skip?: number
    limit?: number
    search?: string
  }): Promise<{
    models: Array<{
      id: string
      name: string
      display_name: string | null
      is_active: boolean
      default_price_per_request: number | null
      default_tiered_pricing: TieredPricingConfig | null
      supported_capabilities: string[] | null
      config: Record<string, unknown> | null
      usage_count: number
    }>
    total: number
  }> {
    const response = await apiClient.get('/api/users/me/available-models', { params })
    return response.data
  },

  // 获取端点状态（不包含敏感信息）
  async getEndpointStatus(): Promise<Array<Record<string, unknown>>> {
    const response = await apiClient.get('/api/users/me/endpoint-status')
    return response.data
  },

  // 偏好设置
  async getPreferences(): Promise<UserPreferences> {
    const response = await apiClient.get('/api/users/me/preferences')
    return response.data
  },

  async updatePreferences(data: Partial<UserPreferences>): Promise<{ message: string }> {
    const response = await apiClient.put('/api/users/me/preferences', data)
    return response.data
  },

  // 提供商绑定管理相关方法已移除，改为直接从可用提供商中选择

  // API密钥提供商关联
  async updateApiKeyProviders(keyId: string, data: {
    allowed_providers?: ProviderConfig[]
  }): Promise<{ message: string }> {
    const response = await apiClient.put(`/api/users/me/api-keys/${keyId}/providers`, data)
    return response.data
  },

  // API密钥能力配置
  async updateApiKeyCapabilities(keyId: string, data: {
    force_capabilities?: Record<string, boolean> | null
  }): Promise<{ message: string; force_capabilities?: Record<string, boolean> | null }> {
    const response = await apiClient.put(`/api/users/me/api-keys/${keyId}/capabilities`, data)
    return response.data
  },

  // 模型能力配置
  async getModelCapabilitySettings(): Promise<{
    model_capability_settings: Record<string, Record<string, boolean>>
  }> {
    const response = await apiClient.get('/api/users/me/model-capabilities')
    return response.data
  },

  async updateModelCapabilitySettings(data: {
    model_capability_settings: Record<string, Record<string, boolean>> | null
  }): Promise<{
    message: string
    model_capability_settings: Record<string, Record<string, boolean>> | null
  }> {
    const response = await apiClient.put('/api/users/me/model-capabilities', data)
    return response.data
  },

  // 获取请求间隔时间线（用于散点图）
  async getIntervalTimeline(params?: {
    hours?: number
    limit?: number
  }): Promise<{
    analysis_period_hours: number
    total_points: number
    points: Array<{ x: string; y: number; model?: string }>
    models?: string[]
  }> {
    const cacheKey = buildCacheKey('me:interval-timeline', params as Record<string, unknown> | undefined)
    return cachedRequest(
      cacheKey,
      async () => {
        const response = await apiClient.get('/api/users/me/usage/interval-timeline', { params })
        return response.data
      },
      30000
    )
  },

  /**
   * 获取活跃度热力图数据（用户）
   * 后端已缓存5分钟
   */
  async getActivityHeatmap(): Promise<ActivityHeatmap> {
    return cachedRequest(
      'me-activity-heatmap',
      async () => {
        const response = await apiClient.get<ActivityHeatmap>('/api/users/me/usage/heatmap')
        return response.data
      },
      60000
    )
  }
}
