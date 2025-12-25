import { defineStore } from 'pinia'
import { ref } from 'vue'
import { usersApi, type User, type CreateUserRequest, type UpdateUserRequest, type ApiKey } from '@/api/users'

export const useUsersStore = defineStore('users', () => {
  const users = ref<User[]>([])
  const loading = ref(false)
  const error = ref<string | null>(null)

  async function fetchUsers() {
    loading.value = true
    error.value = null

    try {
      users.value = await usersApi.getAllUsers()
    } catch (err: any) {
      error.value = err.response?.data?.error?.message || err.response?.data?.detail || '获取用户列表失败'
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
    } catch (err: any) {
      error.value = err.response?.data?.error?.message || err.response?.data?.detail || '创建用户失败'
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
    } catch (err: any) {
      error.value = err.response?.data?.error?.message || err.response?.data?.detail || '更新用户失败'
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
    } catch (err: any) {
      error.value = err.response?.data?.error?.message || err.response?.data?.detail || '删除用户失败'
      throw err
    } finally {
      loading.value = false
    }
  }

  async function getUserApiKeys(userId: string): Promise<ApiKey[]> {
    try {
      return await usersApi.getUserApiKeys(userId)
    } catch (err: any) {
      error.value = err.response?.data?.error?.message || err.response?.data?.detail || '获取 API Keys 失败'
      throw err
    }
  }

  async function createApiKey(userId: string, name?: string): Promise<ApiKey> {
    try {
      return await usersApi.createApiKey(userId, name)
    } catch (err: any) {
      error.value = err.response?.data?.error?.message || err.response?.data?.detail || '创建 API Key 失败'
      throw err
    }
  }

  async function deleteApiKey(userId: string, keyId: string) {
    try {
      await usersApi.deleteApiKey(userId, keyId)
    } catch (err: any) {
      error.value = err.response?.data?.error?.message || err.response?.data?.detail || '删除 API Key 失败'
      throw err
    }
  }

  async function resetUserQuota(userId: string) {
    loading.value = true
    error.value = null

    try {
      await usersApi.resetUserQuota(userId)
      // 刷新用户列表以获取最新数据
      await fetchUsers()
    } catch (err: any) {
      error.value = err.response?.data?.error?.message || err.response?.data?.detail || '重置配额失败'
      throw err
    } finally {
      loading.value = false
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
    deleteApiKey,
    resetUserQuota
  }
})