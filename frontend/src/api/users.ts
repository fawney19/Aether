import apiClient from './client'

export interface User {
  id: string // UUID
  username: string
  email: string
  role: 'admin' | 'user'
  is_active: boolean
  unlimited: boolean
  group_id?: string | null
  group_name?: string | null
  effective_allowed_providers?: string[] | null
  effective_allowed_api_formats?: string[] | null
  effective_allowed_models?: string[] | null
  effective_rate_limit?: number | null
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
  group_id?: string | null
}

export interface UpdateUserRequest {
  username?: string
  email?: string
  is_active?: boolean
  role?: 'admin' | 'user'
  unlimited?: boolean
  password?: string
  group_id?: string | null
}

export interface UserGroup {
  id: string
  name: string
  description?: string | null
  is_default: boolean
  allowed_providers: string[] | null
  allowed_api_formats: string[] | null
  allowed_models: string[] | null
  rate_limit?: number | null
  user_count: number
  created_at: string
  updated_at?: string | null
}

export interface CreateUserGroupRequest {
  name: string
  description?: string | null
  allowed_providers?: string[] | null
  allowed_api_formats?: string[] | null
  allowed_models?: string[] | null
  rate_limit?: number | null
}

export type UpdateUserGroupRequest = Partial<CreateUserGroupRequest>

export interface BatchUserGroupBindingRequest {
  action: 'bind' | 'unbind'
  user_ids: string[]
  group_id?: string | null
  source_group_id?: string | null
}

export interface BatchUserGroupBindingResponse {
  action: 'bind' | 'unbind'
  group_id?: string | null
  source_group_id?: string | null
  updated_count: number
  skipped_count: number
  users: User[]
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
  rate_limit?: number | null  // 普通Key: 0 = 不限制，历史 null 视为跟随系统默认
  total_requests?: number  // 总请求数
  total_cost_usd?: number  // 总费用
}

export interface UpsertUserApiKeyRequest {
  name?: string
  rate_limit?: number | null
}

export type { UserSession } from '@/types/session'

export const usersApi = {
  async getAllUsers(): Promise<User[]> {
    const response = await apiClient.get<User[]>('/api/admin/users')
    return response.data
  },

  async getUser(userId: string): Promise<User> {
    const response = await apiClient.get<User>(`/api/admin/users/${userId}`)
    return response.data
  },

  async getAllUserGroups(): Promise<UserGroup[]> {
    const response = await apiClient.get<UserGroup[]>('/api/admin/users/groups')
    return response.data
  },

  async createUserGroup(data: CreateUserGroupRequest): Promise<UserGroup> {
    const response = await apiClient.post<UserGroup>('/api/admin/users/groups', data)
    return response.data
  },

  async updateUserGroup(groupId: string, data: UpdateUserGroupRequest): Promise<UserGroup> {
    const response = await apiClient.put<UserGroup>(`/api/admin/users/groups/${groupId}`, data)
    return response.data
  },

  async deleteUserGroup(groupId: string): Promise<void> {
    await apiClient.delete(`/api/admin/users/groups/${groupId}`)
  },

  async batchUpdateUserGroupBinding(
    data: BatchUserGroupBindingRequest
  ): Promise<BatchUserGroupBindingResponse> {
    const response = await apiClient.post<BatchUserGroupBindingResponse>(
      '/api/admin/users/groups/bindings/batch',
      data
    )
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

  async getUserSessions(userId: string): Promise<UserSession[]> {
    const response = await apiClient.get<UserSession[]>(`/api/admin/users/${userId}/sessions`)
    return response.data
  },

  async revokeUserSession(userId: string, sessionId: string): Promise<{ message: string }> {
    const response = await apiClient.delete(`/api/admin/users/${userId}/sessions/${sessionId}`)
    return response.data
  },

  async revokeAllUserSessions(userId: string): Promise<{ message: string; revoked_count: number }> {
    const response = await apiClient.delete(`/api/admin/users/${userId}/sessions`)
    return response.data
  },

  async createApiKey(
    userId: string,
    data: UpsertUserApiKeyRequest
  ): Promise<ApiKey & { key: string }> {
    const response = await apiClient.post<ApiKey & { key: string }>(
      `/api/admin/users/${userId}/api-keys`,
      data
    )
    return response.data
  },

  async updateApiKey(
    userId: string,
    keyId: string,
    data: UpsertUserApiKeyRequest
  ): Promise<ApiKey & { message: string }> {
    const response = await apiClient.put<ApiKey & { message: string }>(
      `/api/admin/users/${userId}/api-keys/${keyId}`,
      data
    )
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
  }
}
