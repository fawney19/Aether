import { defineStore } from 'pinia'
import { ref } from 'vue'
import {
  usersApi,
  type User,
  type CreateUserRequest,
  type UpdateUserRequest,
  type ApiKey,
  type UpsertUserApiKeyRequest,
  type UserSession,
} from '@/api/users'
import { parseApiError } from '@/utils/errorParser'

export const useUsersStore = defineStore('users', () => {
  const users = ref<User[]>([])
  const loading = ref(false)
  const error = ref<string | null>(null)

  async function fetchUsers() {
    loading.value = true
    error.value = null

    try {
      users.value = await usersApi.getAllUsers()
    } catch (err: unknown) {
      error.value = parseApiError(err, '获取用户列表失败')
    } finally {
      loading.value = false
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
    loading,
    error,
    fetchUsers,
    createUser,
    updateUser,
    deleteUser,
    getUserApiKeys,
    createApiKey,
    updateApiKey,
    deleteApiKey,
    getFullApiKey,
    getUserSessions,
    revokeUserSession,
    revokeAllUserSessions,
  }
})
