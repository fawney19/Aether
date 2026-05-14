import apiClient from './client'
import { cachedRequest } from '@/utils/cache'
import type { UserSession as SessionRecord } from '@/types/session'

export type UserRole = 'admin' | 'user'
export type ListPolicyMode = 'inherit' | 'unrestricted' | 'specific' | 'deny_all'
export type RateLimitPolicyMode = 'inherit' | 'system' | 'custom'
export type FeatureSettings = Record<string, unknown>

export interface UserGroupSummary {
  id: string
  name: string
}

export interface EffectivePolicyField<T> {
  mode: string
  value: T | null
  source: 'user' | 'group' | 'fallback' | string
  group_id?: string | null
  group_name?: string | null
  group_ids?: string[]
  group_names?: string[]
}

export interface UserEffectivePolicy {
  allowed_providers?: EffectivePolicyField<string[]>
  allowed_api_formats?: EffectivePolicyField<string[]>
  allowed_models?: EffectivePolicyField<string[]>
  rate_limit?: EffectivePolicyField<number>
}

export interface User {
  id: string // UUID
  username: string
  email: string
  role: UserRole
  is_active: boolean
  unlimited: boolean
  feature_settings?: FeatureSettings | null
  allowed_providers: string[] | null  // 允许使用的提供商 ID 列表
  allowed_providers_mode?: ListPolicyMode
  allowed_api_formats: string[] | null  // 允许使用的 API 格式列表
  allowed_api_formats_mode?: ListPolicyMode
  allowed_models: string[] | null  // 允许使用的模型名称列表
  allowed_models_mode?: ListPolicyMode
  rate_limit?: number | null  // null = 跟随系统默认，0 = 不限制
  rate_limit_mode?: RateLimitPolicyMode
  groups?: UserGroupSummary[]
  effective_policy?: UserEffectivePolicy
  created_at: string
  updated_at?: string
  last_login_at?: string | null
  request_count?: number
  total_tokens?: number
}

export interface CreateUserRequest {
  username: string
  password: string
  email: string
  role?: UserRole
  initial_gift_usd?: number | null
  unlimited?: boolean
  group_ids?: string[]
  feature_settings?: FeatureSettings | null
}

export interface UpdateUserRequest {
  email?: string
  is_active?: boolean
  role?: UserRole
  unlimited?: boolean
  password?: string
  group_ids?: string[]
  feature_settings?: FeatureSettings | null
}

export interface UserBatchSelectionFilters {
  search?: string
  role?: UserRole
  is_active?: boolean
  group_id?: string
}

export interface UserBatchSelection {
  user_ids?: string[]
  group_ids?: string[]
  filters?: UserBatchSelectionFilters | null
}

export interface UserBatchSelectionItem {
  user_id: string
  username: string
  email?: string | null
  role: UserRole
  is_active: boolean
  matched_by?: string[]
}

export interface UserBatchSelectionWarning {
  type: string
  group_id?: string | null
  message: string
}

export interface ResolveUserBatchSelectionResponse {
  total: number
  items: UserBatchSelectionItem[]
  warnings?: UserBatchSelectionWarning[]
}

export interface UserBatchAccessControlPayload {
  unlimited?: boolean
}

export interface UserBatchRolePayload {
  role: UserRole
}

export type UserBatchAction = 'enable' | 'disable' | 'update_access_control' | 'update_role'

export type UserBatchActionPayload = UserBatchAccessControlPayload | UserBatchRolePayload

export interface UserBatchToggleActionRequest {
  selection: UserBatchSelection
  action: 'enable' | 'disable'
  payload?: null
}

export interface UserBatchAccessControlActionRequest {
  selection: UserBatchSelection
  action: 'update_access_control'
  payload: UserBatchAccessControlPayload
}

export interface UserBatchRoleActionRequest {
  selection: UserBatchSelection
  action: 'update_role'
  payload: UserBatchRolePayload
}

export type UserBatchActionRequest =
  | UserBatchToggleActionRequest
  | UserBatchAccessControlActionRequest
  | UserBatchRoleActionRequest

export interface UserBatchActionFailure {
  user_id: string
  reason: string
}

export interface UserBatchActionResponse {
  total: number
  success: number
  failed: number
  failures: UserBatchActionFailure[]
  warnings?: UserBatchSelectionWarning[]
  action?: string
  modified_fields?: string[]
}

export interface UserGroup {
  id: string
  name: string
  normalized_name?: string
  description?: string | null
  allowed_providers?: string[] | null
  allowed_providers_mode: ListPolicyMode
  allowed_api_formats?: string[] | null
  allowed_api_formats_mode: ListPolicyMode
  allowed_models?: string[] | null
  allowed_models_mode: ListPolicyMode
  rate_limit?: number | null
  rate_limit_mode: RateLimitPolicyMode
  is_default?: boolean
  created_at?: string | null
  updated_at?: string | null
}

export interface UpsertUserGroupRequest {
  name: string
  description?: string | null
  allowed_providers?: string[] | null
  allowed_providers_mode?: ListPolicyMode
  allowed_api_formats?: string[] | null
  allowed_api_formats_mode?: ListPolicyMode
  allowed_models?: string[] | null
  allowed_models_mode?: ListPolicyMode
  rate_limit?: number | null
  rate_limit_mode?: RateLimitPolicyMode
}

export interface UserGroupMember {
  group_id: string
  user_id: string
  username: string
  email?: string | null
  role: UserRole
  is_active: boolean
  is_deleted: boolean
  created_at?: string | null
}

export interface ListUserGroupsResponse {
  items: UserGroup[]
  default_group_id?: string | null
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
  feature_settings?: FeatureSettings | null
  rate_limit?: number | null  // 普通Key: 0 = 不限制，历史 null 视为跟随系统默认
  concurrent_limit?: number | null  // 普通Key: 0 = 不限制并发，历史 null 兼容
  total_requests?: number  // 总请求数
  total_cost_usd?: number  // 总费用
}

export interface UpsertUserApiKeyRequest {
  name?: string
  rate_limit?: number | null
  concurrent_limit?: number | null
  feature_settings?: FeatureSettings | null
}

export type UserSession = SessionRecord

export interface GetAllUsersOptions {
  search?: string
  role?: UserRole
  is_active?: boolean
  group_id?: string
  skip?: number
  limit?: number
  cacheTtlMs?: number
}

export const usersApi = {
  async getAllUsers(options: GetAllUsersOptions = {}): Promise<User[]> {
    const cacheTtlMs = options.cacheTtlMs ?? 0
    const params: Record<string, string | number> = {}
    const search = options.search?.trim()

    if (search) params.search = search
    if (options.role) params.role = options.role
    if (options.is_active !== undefined) params.is_active = options.is_active ? 'true' : 'false'
    if (options.group_id) params.group_id = options.group_id
    if (options.skip !== undefined) params.skip = options.skip
    if (options.limit !== undefined) params.limit = options.limit

    const cacheKey = Object.keys(params).length === 0
      ? 'admin:users:list'
      : [
          'admin:users:list',
          search ?? '',
          options.role ?? '',
          options.is_active ?? '',
          options.group_id ?? '',
          options.skip ?? '',
          options.limit ?? '',
        ].join(':')

    return cachedRequest(
      cacheKey,
      async () => {
        const response = await apiClient.get<User[]>('/api/admin/users', {
          params: Object.keys(params).length > 0 ? params : undefined,
        })
        return response.data
      },
      cacheTtlMs,
    )
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

  async resolveBatchSelection(
    selection: UserBatchSelection
  ): Promise<ResolveUserBatchSelectionResponse> {
    const response = await apiClient.post<ResolveUserBatchSelectionResponse>(
      '/api/admin/users/resolve-selection',
      selection
    )
    return response.data
  },

  async batchAction(request: UserBatchActionRequest): Promise<UserBatchActionResponse> {
    const response = await apiClient.post<UserBatchActionResponse>(
      '/api/admin/users/batch-action',
      request
    )
    return response.data
  },

  async listUserGroups(): Promise<ListUserGroupsResponse> {
    const response = await apiClient.get<ListUserGroupsResponse>('/api/admin/user-groups')
    return response.data
  },

  async createUserGroup(payload: UpsertUserGroupRequest): Promise<UserGroup> {
    const response = await apiClient.post<UserGroup>('/api/admin/user-groups', payload)
    return response.data
  },

  async updateUserGroup(groupId: string, payload: UpsertUserGroupRequest): Promise<UserGroup> {
    const response = await apiClient.put<UserGroup>(`/api/admin/user-groups/${groupId}`, payload)
    return response.data
  },

  async deleteUserGroup(groupId: string): Promise<void> {
    await apiClient.delete(`/api/admin/user-groups/${groupId}`)
  },

  async listUserGroupMembers(groupId: string): Promise<UserGroupMember[]> {
    const response = await apiClient.get<{ items: UserGroupMember[] }>(`/api/admin/user-groups/${groupId}/members`)
    return response.data.items
  },

  async replaceUserGroupMembers(groupId: string, userIds: string[]): Promise<UserGroupMember[]> {
    const response = await apiClient.put<{ items: UserGroupMember[] }>(
      `/api/admin/user-groups/${groupId}/members`,
      { user_ids: userIds },
    )
    return response.data.items
  },

  async setDefaultUserGroup(groupId: string | null): Promise<{ default_group_id?: string | null }> {
    const response = await apiClient.put<{ default_group_id?: string | null }>(
      '/api/admin/user-groups/default',
      { group_id: groupId },
    )
    return response.data
  },

  async deleteUser(userId: string): Promise<void> {
    await apiClient.delete(`/api/admin/users/${userId}`)
  },

  async getUserApiKeys(userId: string): Promise<ApiKey[]> {
    const response = await apiClient.get<{ api_keys: ApiKey[] }>(`/api/admin/users/${userId}/api-keys`)
    return response.data.api_keys
  },

  async getUserSessions(userId: string): Promise<SessionRecord[]> {
    const response = await apiClient.get<SessionRecord[]>(`/api/admin/users/${userId}/sessions`)
    return response.data
  },

  async revokeUserSession(userId: string, sessionId: string): Promise<{ message: string }> {
    const response = await apiClient.delete<{ message: string }>(`/api/admin/users/${userId}/sessions/${sessionId}`)
    return response.data
  },

  async revokeAllUserSessions(userId: string): Promise<{ message: string; revoked_count: number }> {
    const response = await apiClient.delete<{ message: string; revoked_count: number }>(`/api/admin/users/${userId}/sessions`)
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
  },
  // 管理员统计
  async getUsageStats(): Promise<Record<string, unknown>> {
    const response = await apiClient.get<Record<string, unknown>>('/api/admin/usage/stats')
    return response.data
  }
}
