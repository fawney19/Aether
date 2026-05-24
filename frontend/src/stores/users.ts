import { defineStore } from 'pinia'
import { ref } from 'vue'
import {
  usersApi,
  type User,
  type GetAllUsersOptions,
  type CreateUserRequest,
  type UpdateUserRequest,
  type ApiKey,
  type UpsertUserApiKeyRequest,
  type UserSession,
  type UserBatchSelection,
  type ResolveUserBatchSelectionResponse,
  type UserBatchActionRequest,
  type UserBatchActionResponse,
  type UserRole,
  type UserGroup,
  type UserGroupMember,
  type UpsertUserGroupRequest,
  type ListUserGroupsResponse,
  type AdminUserPlanEntitlementsResponse,
  type GrantUserPlanRequest,
  type GrantUserPlanResponse,
} from '@/api/users'
import { parseApiError } from '@/utils/errorParser'

export const useUsersStore = defineStore('users', () => {
  const users = ref<User[]>([])
  const total = ref(0)
  const skip = ref(0)
  const limit = ref(0)
  const hasMore = ref(false)
  const loading = ref(false)
  const error = ref<string | null>(null)
  let fetchUsersRequestId = 0

  async function fetchUsers(options: {
    cacheTtlMs?: number
    search?: string
    role?: UserRole
    is_active?: boolean
    group_id?: string
    skip?: number
    limit?: number
  } = {}) {
    const requestId = ++fetchUsersRequestId
    loading.value = true
    error.value = null

    try {
      const response = await usersApi.getAllUsersPage(options)
      if (requestId !== fetchUsersRequestId) return
      users.value = response.items
      total.value = response.total
      skip.value = response.skip
      limit.value = response.limit
      hasMore.value = response.has_more
    } catch (err: unknown) {
      if (requestId !== fetchUsersRequestId) return
      error.value = parseApiError(err, '获取用户列表失败')
    } finally {
      if (requestId === fetchUsersRequestId) {
        loading.value = false
      }
    }
  }

  async function listAllUsers(options: Omit<GetAllUsersOptions, 'skip' | 'limit'> = {}): Promise<User[]> {
    const pageSize = 1000
    const allUsers: User[] = []
    let skip = 0

    for (;;) {
      const response = await usersApi.getAllUsersPage({
        ...options,
        skip,
        limit: pageSize,
      })
      allUsers.push(...response.items)
      if (!response.has_more || response.items.length === 0) {
        return allUsers
      }
      skip += response.items.length
    }
  }

  async function createUser(userData: CreateUserRequest) {
    loading.value = true
    error.value = null

    try {
      const newUser = await usersApi.createUser(userData)
      users.value.push(newUser)
      return newUser
    } catch (err: unknown) {
      error.value = parseApiError(err, '创建用户失败')
      throw err
    } finally {
      loading.value = false
    }
  }

  async function updateUser(userId: string, updates: UpdateUserRequest) {
    loading.value = true
    error.value = null

    try {
      const updatedUser = await usersApi.updateUser(userId, updates)
      const index = users.value.findIndex(u => u.id === userId)
      if (index !== -1) {
        // 保留原有的创建时间等字段，只更新返回的字段
        users.value[index] = {
          ...users.value[index],
          ...updatedUser
        }
      }
      return updatedUser
    } catch (err: unknown) {
      error.value = parseApiError(err, '更新用户失败')
      throw err
    } finally {
      loading.value = false
    }
  }

  async function deleteUser(userId: string) {
    loading.value = true
    error.value = null

    try {
      await usersApi.deleteUser(userId)
      users.value = users.value.filter(u => u.id !== userId)
    } catch (err: unknown) {
      error.value = parseApiError(err, '删除用户失败')
      throw err
    } finally {
      loading.value = false
    }
  }

  async function resolveBatchSelection(
    selection: UserBatchSelection
  ): Promise<ResolveUserBatchSelectionResponse> {
    try {
      return await usersApi.resolveBatchSelection(selection)
    } catch (err: unknown) {
      error.value = parseApiError(err, '解析用户选择失败')
      throw err
    }
  }

  async function batchAction(request: UserBatchActionRequest): Promise<UserBatchActionResponse> {
    loading.value = true
    error.value = null
    try {
      return await usersApi.batchAction(request)
    } catch (err: unknown) {
      error.value = parseApiError(err, '批量操作用户失败')
      throw err
    } finally {
      loading.value = false
    }
  }

  async function listUserGroups(): Promise<ListUserGroupsResponse> {
    try {
      return await usersApi.listUserGroups()
    } catch (err: unknown) {
      error.value = parseApiError(err, '获取用户分组失败')
      throw err
    }
  }

  async function createUserGroup(payload: UpsertUserGroupRequest): Promise<UserGroup> {
    try {
      return await usersApi.createUserGroup(payload)
    } catch (err: unknown) {
      error.value = parseApiError(err, '创建用户分组失败')
      throw err
    }
  }

  async function updateUserGroup(
    groupId: string,
    payload: UpsertUserGroupRequest,
  ): Promise<UserGroup> {
    try {
      return await usersApi.updateUserGroup(groupId, payload)
    } catch (err: unknown) {
      error.value = parseApiError(err, '更新用户分组失败')
      throw err
    }
  }

  async function deleteUserGroup(groupId: string): Promise<void> {
    try {
      await usersApi.deleteUserGroup(groupId)
    } catch (err: unknown) {
      error.value = parseApiError(err, '删除用户分组失败')
      throw err
    }
  }

  async function listUserGroupMembers(groupId: string): Promise<UserGroupMember[]> {
    try {
      return await usersApi.listUserGroupMembers(groupId)
    } catch (err: unknown) {
      error.value = parseApiError(err, '获取分组成员失败')
      throw err
    }
  }

  async function replaceUserGroupMembers(
    groupId: string,
    userIds: string[],
  ): Promise<UserGroupMember[]> {
    try {
      return await usersApi.replaceUserGroupMembers(groupId, userIds)
    } catch (err: unknown) {
      error.value = parseApiError(err, '更新分组成员失败')
      throw err
    }
  }

  async function setDefaultUserGroup(groupId: string | null): Promise<{ default_group_id?: string | null }> {
    try {
      return await usersApi.setDefaultUserGroup(groupId)
    } catch (err: unknown) {
      error.value = parseApiError(err, '设置默认用户组失败')
      throw err
    }
  }

  async function getUserApiKeys(userId: string): Promise<ApiKey[]> {
    try {
      return await usersApi.getUserApiKeys(userId)
    } catch (err: unknown) {
      error.value = parseApiError(err, '获取 API Keys 失败')
      throw err
    }
  }

  async function createApiKey(userId: string, data: UpsertUserApiKeyRequest): Promise<ApiKey> {
    try {
      return await usersApi.createApiKey(userId, data)
    } catch (err: unknown) {
      error.value = parseApiError(err, '创建 API Key 失败')
      throw err
    }
  }

  async function updateApiKey(
    userId: string,
    keyId: string,
    data: UpsertUserApiKeyRequest
  ): Promise<ApiKey> {
    try {
      return await usersApi.updateApiKey(userId, keyId, data)
    } catch (err: unknown) {
      error.value = parseApiError(err, '更新 API Key 失败')
      throw err
    }
  }

  async function deleteApiKey(userId: string, keyId: string) {
    try {
      await usersApi.deleteApiKey(userId, keyId)
    } catch (err: unknown) {
      error.value = parseApiError(err, '删除 API Key 失败')
      throw err
    }
  }

  async function getFullApiKey(userId: string, keyId: string): Promise<{ key: string }> {
    try {
      return await usersApi.getFullApiKey(userId, keyId)
    } catch (err: unknown) {
      error.value = parseApiError(err, '获取完整 API Key 失败')
      throw err
    }
  }

  async function getUserSessions(userId: string): Promise<UserSession[]> {
    try {
      return await usersApi.getUserSessions(userId)
    } catch (err: unknown) {
      error.value = parseApiError(err, '获取用户设备会话失败')
      throw err
    }
  }

  async function listUserPlanEntitlements(
    userId: string,
  ): Promise<AdminUserPlanEntitlementsResponse> {
    try {
      return await usersApi.listUserPlanEntitlements(userId)
    } catch (err: unknown) {
      error.value = parseApiError(err, '获取用户套餐失败')
      throw err
    }
  }

  async function grantUserPlan(
    userId: string,
    payload: GrantUserPlanRequest,
  ): Promise<GrantUserPlanResponse> {
    try {
      return await usersApi.grantUserPlan(userId, payload)
    } catch (err: unknown) {
      error.value = parseApiError(err, '发放用户套餐失败')
      throw err
    }
  }

  async function revokeUserSession(userId: string, sessionId: string): Promise<{ message: string }> {
    try {
      return await usersApi.revokeUserSession(userId, sessionId)
    } catch (err: unknown) {
      error.value = parseApiError(err, '强制下线设备失败')
      throw err
    }
  }

  async function revokeAllUserSessions(
    userId: string,
  ): Promise<{ message: string; revoked_count: number }> {
    try {
      return await usersApi.revokeAllUserSessions(userId)
    } catch (err: unknown) {
      error.value = parseApiError(err, '强制下线全部设备失败')
      throw err
    }
  }

  return {
    users,
    total,
    skip,
    limit,
    hasMore,
    loading,
    error,
    fetchUsers,
    listAllUsers,
    createUser,
    updateUser,
    deleteUser,
    resolveBatchSelection,
    batchAction,
    listUserGroups,
    createUserGroup,
    updateUserGroup,
    deleteUserGroup,
    listUserGroupMembers,
    replaceUserGroupMembers,
    setDefaultUserGroup,
    getUserApiKeys,
    createApiKey,
    updateApiKey,
    deleteApiKey,
    getFullApiKey,
    getUserSessions,
    listUserPlanEntitlements,
    grantUserPlan,
    revokeUserSession,
    revokeAllUserSessions,
  }
})
