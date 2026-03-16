import { defineStore } from 'pinia'
import { ref, computed } from 'vue'
import { authApi, type User } from '@/api/auth'
import apiClient from '@/api/client'
import { log } from '@/utils/logger'
import { parseApiError } from '@/utils/errorParser'
import { getErrorStatus } from '@/types/api-error'

export const useAuthStore = defineStore('auth', () => {
  // 初始化时从 localStorage 恢复 token
  const storedToken = apiClient.getToken()

  const user = ref<User | null>(null)
  const token = ref<string | null>(storedToken)
  const loading = ref(false)
  const error = ref<string | null>(null)

  const isAuthenticated = computed(() => {
    // 使用 store 中的 token 状态判断认证状态
    // 如果需要同步 localStorage，应该在 checkAuth 或专门的 syncToken 方法中处理
    return !!token.value
  })

  /**
   * 同步 localStorage 中的 token 到 store
   * 用于处理多标签页或外部 token 变更的情况
   */
  function syncToken() {
    const currentToken = apiClient.getToken()
    if (token.value !== currentToken) {
      token.value = currentToken
    }
  }
  const isAdmin = computed(() => user.value?.role === 'admin')

  async function login(email: string, password: string, authType: 'local' | 'ldap' = 'local') {
    loading.value = true
    error.value = null

    try {
      const response = await authApi.login({ email, password, auth_type: authType })
      token.value = response.access_token

      // 获取用户信息
      const userInfo = await authApi.getCurrentUser()
      user.value = userInfo

      return true
    } catch (err: unknown) {
      // 不要暴露后端的详细错误信息
      const status = getErrorStatus(err)
      if (status === 401) {
        error.value = '邮箱或密码错误'
      } else if (status === 422) {
        error.value = '请输入有效的邮箱地址'
      } else if (status === 429) {
        // 限流错误，显示后端返回的具体信息
        error.value = parseApiError(err, '请求过于频繁,请稍后重试')
      } else if (status === 500) {
        error.value = '服务器错误,请稍后重试'
      } else {
        error.value = '登录失败,请检查网络连接'
      }
      return false
    } finally {
      loading.value = false
    }
  }

  async function logout() {
    user.value = null
    token.value = null
    await authApi.logout()
  }

  function applyExternalLogout() {
    user.value = null
    token.value = null
    error.value = null
  }

  async function fetchCurrentUser() {
    try {
      const userInfo = await authApi.getCurrentUser()
      user.value = userInfo
      return userInfo
    } catch (err: unknown) {
      log.error('Failed to fetch user info', err)
      syncToken()
      if (!token.value) {
        user.value = null
      }
      // 根据用户要求,不管什么错误都不清除状态
      // 保持登录状态,除非用户手动退出
      log.info('Keeping session despite error, as per user requirement')
      return null
    }
  }

  async function checkAuth() {
    const storedToken = apiClient.getToken()
    if (storedToken) {
      token.value = storedToken
      // 即使获取用户信息失败,也保留 token
      // 只有 401 错误才表示 token 真正失效
      await fetchCurrentUser()
    }
  }

  return {
    user,
    token,
    loading,
    error,
    isAuthenticated,
    isAdmin,
    login,
    logout,
    applyExternalLogout,
    fetchCurrentUser,
    checkAuth,
    syncToken
  }
})
