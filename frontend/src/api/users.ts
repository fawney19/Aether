import apiClient from './client'

export interface User {
  id: string // UUID
  username: string
  email: string
  role: 'admin' | 'user'
  is_active: boolean
  unlimited: boolean
  allowed_providers: string[] | null  // 允许使用的提供商 ID 列表
  allowed_api_formats: string[] | null  // 允许使用的 API 格式列表
  allowed_models: string[] | null  // 允许使用的模型名称列表
  created_at: string
  updated_at?: string
  last_login_at?: string | null
}

export interface CreateUserRequest {
  username: string
  password: string
  email: string
  role?: 'admin' | 'user'
  initial_gift_usd?: number | null
  unlimited?: boolean
  allowed_providers?: string[] | null
  allowed_api_formats?: string[] | null
  allowed_models?: string[] | null
}

export interface UpdateUserRequest {
  email?: string
  is_active?: boolean
  role?: 'admin' | 'user'
  unlimited?: boolean
  password?: string
  allowed_providers?: string[] | null
  allowed_api_formats?: string[] | null
  allowed_models?: string[] | null
}

export interface ApiKey {
  id: string // UUID
  key?: string  // 完整的 key，只在创建时返回
  key_display?: string  // 脱敏后的密钥显示
  name?: string
  created_at: string
  last_used_at?: string
  expires_at?: string  // 过期时间
  is_active: boolean
  is_locked: boolean  // 管理员锁定标志
  is_standalone: boolean  // 是否为独立余额Key
  rate_limit?: number  // 速率限制（请求/分钟）
  total_requests?: number  // 总请求数
  total_cost_usd?: number  // 总费用
}

export const usersApi = {
  async getAllUsers(): Promise<User[]> {
    const response = await apiClient.get<User[]>('/api/admin/users')
    return response.data
  },

  async getUser(userId: string): Promise<User> {
    const response = await apiClient.get<User>(`/api/admin/users/${userId}`)
    return response.data
  },

  async createUser(user: CreateUserRequest): Promise<User> {
    const response = await apiClient.post<User>('/api/admin/users', user)
    return response.data
  },

  async updateUser(userId: string, updates: UpdateUserRequest): Promise<User> {
    const response = await apiClient.put<User>(`/api/admin/users/${userId}`, updates)
    return response.data
  },

  async deleteUser(userId: string): Promise<void> {
    await apiClient.delete(`/api/admin/users/${userId}`)
  },

  async getUserApiKeys(userId: string): Promise<ApiKey[]> {
    const response = await apiClient.get<{ api_keys: ApiKey[] }>(`/api/admin/users/${userId}/api-keys`)
    return response.data.api_keys
  },

  async createApiKey(userId: string, name?: string): Promise<ApiKey & { key: string }> {
    const response = await apiClient.post<ApiKey & { key: string }>(`/api/admin/users/${userId}/api-keys`, { name })
    return response.data
  },

  async deleteApiKey(userId: string, keyId: string): Promise<void> {
    await apiClient.delete(`/api/admin/users/${userId}/api-keys/${keyId}`)
  },

  async getFullApiKey(userId: string, keyId: string): Promise<{ key: string }> {
    const response = await apiClient.get<{ key: string }>(
      `/api/admin/users/${userId}/api-keys/${keyId}/full-key`
    )
    return response.data
  },
  // 管理员统计
  async getUsageStats(): Promise<Record<string, unknown>> {
    const response = await apiClient.get('/api/admin/usage/stats')
    return response.data
  }
}
