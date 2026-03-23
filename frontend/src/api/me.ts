import apiClient from './client'
import type { TieredPricingConfig } from './endpoints/types'
import type { BillingSummary } from './auth'

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
  theme: string
  language: string
  timezone?: string
  notifications?: {
    email?: boolean
    usage_alerts?: boolean
    announcements?: boolean
  }
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

}
